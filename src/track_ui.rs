//! "Track" tab: open a track (.ptf, or the one inside a .dat) and look at it, from above or
//! in 3D, with every piece of track listed in plain numbers. Read-only for now.

use eframe::egui::{self, Color32, RichText, Sense};
use eframe::egui_glow;
use eframe::glow::{self, HasContext};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use ttk::config::Settings;

mod saving;
use ttk::track::mesh::{self, TrackMesh, VERTEX_FLOATS};
use ttk::track::model::{self, surface_name, wall_name, Track, FEET_PER_METRE, METRES_PER_MILE};
use ttk::track::graph;

pub struct TrackEnv<'a> {
    /// (name, folder) of tracks installed in the game
    pub game_tracks: &'a [(String, PathBuf)],
    /// The workspace `tracks` folder (where Save first suggests putting a track from a .dat)
    pub workspace: &'a Path,
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Map,
    ThreeD,
}

struct Loaded {
    name: String,
    source: String,
    graph: graph::Graph,
    track: Track,
    mesh: TrackMesh,
    /// Where Save writes: the .ptf it came from, or None for a track read out of a .dat.
    save_path: Option<PathBuf>,
    /// An earlier version (backup) opened to be put back.
    restored: bool,
}

struct Camera {
    // map (top-down): centre and half height of the visible area, in mesh metres
    cx: f64,
    cy: f64,
    half_h: f64,
    // 3D orbit
    target: [f64; 3],
    yaw: f64,
    pitch: f64,
    dist: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self { cx: 0.0, cy: 0.0, half_h: 500.0, target: [0.0; 3], yaw: -1.9, pitch: 0.6, dist: 1000.0 }
    }
}

pub struct TrackView {
    picked: Option<String>,
    loading: Option<mpsc::Receiver<Result<Loaded, String>>>,
    loaded: Option<Loaded>,
    error: Option<String>,
    view: View,
    cam: Camera,
    selected: Option<usize>,
    show_walls: bool,
    show_objects: bool,
    /// Scroll the piece list to the selection on the next frame (after a map click).
    scroll_list: bool,
    /// Viewport width / height, from the last frame.
    aspect: f64,
    /// Piece to select once the track has loaded (command line `--piece N`).
    select_on_load: Option<usize>,
    /// Fit the camera on the next frame (needs the viewport size).
    fit_pending: bool,
    ed: saving::EditState,
    workspace: Option<PathBuf>,
    scene: Arc<Mutex<Scene>>,
}

impl Default for TrackView {
    fn default() -> Self {
        Self {
            picked: None,
            loading: None,
            loaded: None,
            error: None,
            view: View::Map,
            cam: Camera::default(),
            selected: None,
            show_walls: true,
            show_objects: true,
            scroll_list: false,
            aspect: 1.4,
            select_on_load: None,
            fit_pending: false,
            ed: Default::default(),
            workspace: None,
            scene: Arc::new(Mutex::new(Scene::default())),
        }
    }
}

impl TrackView {
    /// Load a track in the background. `restore_to`: `path` is an earlier version of that file.
    fn open(&mut self, ctx: &egui::Context, name: String, path: PathBuf, restore_to: Option<PathBuf>) {
        let (tx, rx) = mpsc::channel();
        let (walls, objects) = (self.show_walls, self.show_objects);
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let r = (|| -> anyhow::Result<Loaded> {
                // Backups are named <file>.ptf.<timestamp>, so read those as .ptf whatever the name.
                let (bytes, mut source) = if restore_to.is_some() {
                    (std::fs::read(&path)?, path.display().to_string())
                } else {
                    graph::load_ptf_bytes(&path)?
                };
                let g = graph::parse_ptf(&bytes)?;
                let track = Track::from_graph(&g)?;
                let mesh = mesh::build(&track, walls, objects);
                let restored = restore_to.is_some();
                let save_path = restore_to.or_else(|| {
                    (ttk::util::ext_lower(&path) == "ptf").then(|| path.clone())
                });
                if restored {
                    source = format!("earlier version: {source}");
                }
                Ok(Loaded { name, source, graph: g, track, mesh, save_path, restored })
            })();
            let _ = tx.send(r.map_err(|e| format!("{e:#}")));
            ctx2.request_repaint();
        });
        self.loading = Some(rx);
        self.error = None;
    }

    /// Open a file directly (command line).
    pub fn open_path(&mut self, ctx: &egui::Context, path: PathBuf, three_d: bool, piece: Option<usize>) {
        self.select_on_load = piece;
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        self.picked = Some(name.clone());
        if three_d {
            self.view = View::ThreeD;
        }
        self.open(ctx, name, path, None);
    }

    fn poll(&mut self) {
        let Some(rx) = &self.loading else { return };
        match rx.try_recv() {
            Ok(Ok(mut l)) => {
                self.scene.lock().unwrap().pending = Some(std::mem::take(&mut l.mesh.verts));
                self.selected = None;
                let restored = l.restored;
                self.loaded = Some(l);
                self.reset_edits(restored);
                self.loading = None;
                self.fit_pending = true;
            }
            Ok(Err(e)) => {
                self.error = Some(e);
                self.loading = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.error = Some("reading the track stopped unexpectedly".into());
                self.loading = None;
            }
        }
    }

    fn rebuild_mesh(&mut self) {
        if let Some(l) = &mut self.loaded {
            let mut m = mesh::build(&l.track, self.show_walls, self.show_objects);
            self.scene.lock().unwrap().pending = Some(std::mem::take(&mut m.verts));
            l.mesh = m;
        }
    }

    fn fit(&mut self) {
        if let Some(l) = &self.loaded {
            let [w, h, z0, z1] = l.mesh.extent;
            self.cam.cx = 0.0;
            self.cam.cy = 0.0;
            // fit both directions of the viewport (aspect = width / height)
            self.cam.half_h = 0.55 * h.max(w / self.aspect.max(0.1)).max(20.0);
            self.cam.target = [0.0, 0.0, 0.5 * (z1 - z0)];
            self.cam.dist = 1.1 * w.max(h).max(20.0);
            self.cam.yaw = -1.9;
            self.cam.pitch = 0.6;
        }
    }

    fn focus_segment(&mut self, k: usize) {
        if let Some(l) = &self.loaded {
            let s = &l.track.segments[k];
            let p = s.point(0.5, 0.0);
            let o = l.mesh.origin;
            self.cam.cx = p[0] - o[0];
            self.cam.cy = p[1] - o[1];
            self.cam.target = [p[0] - o[0], p[1] - o[1], s.height(0.5, 0.0) - o[2]];
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, env: &TrackEnv, settings: &mut Settings) {
        self.poll();
        self.workspace = Some(env.workspace.to_path_buf());
        if self.loaded.is_some() && self.ed.pending.is_none() {
            self.shortcuts(ctx);
        }
        egui::SidePanel::left("track_side").resizable(true).default_width(370.0).show(ctx, |ui| {
            self.side_panel(ui, ctx, env, settings);
        });
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
            self.viewport(ui);
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, env: &TrackEnv, settings: &mut Settings) {
        ui.add_space(6.0);
        ui.label(RichText::new("Look at a track").size(18.0).strong());
        ui.label("Pick a track from the game, or open a .ptf or .dat file.");
        ui.add_space(4.0);
        let mut open: Option<(String, PathBuf)> = None;
        ui.horizontal(|ui| {
            let text = self.picked.clone().unwrap_or_else(|| "Choose a track…".into());
            egui::ComboBox::from_id_salt("track_view_pick").selected_text(text).width(200.0).height(420.0).show_ui(
                ui,
                |ui| {
                    for (name, folder) in env.game_tracks.iter().filter(|(n, _)| !n.eq_ignore_ascii_case("shared")) {
                        if ui.selectable_label(self.picked.as_deref() == Some(name.as_str()), name).clicked() {
                            match graph::track_file_in(folder) {
                                Some(p) => open = Some((name.clone(), p)),
                                None => self.error = Some(format!("{name} has no .ptf or .dat file")),
                            }
                        }
                    }
                },
            );
            if ui.button("Open a file…").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Track", &["ptf", "dat"]).pick_file() {
                    let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    open = Some((name, p));
                }
            }
        });
        if let Some((name, path)) = open {
            self.request_open(ctx, name, path);
        }
        if self.loading.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the track…");
            });
        }
        if let Some(e) = &self.error {
            ui.colored_label(Color32::from_rgb(230, 90, 80), e);
        }
        let Some(l) = &self.loaded else { return };

        ui.separator();
        let t = &l.track;
        let curves = t.segments.iter().filter(|s| s.curve).count();
        egui::Grid::new("track_facts").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            ui.label("Track");
            ui.label(RichText::new(&l.name).strong());
            ui.end_row();
            ui.label("Length");
            ui.label(format!("{:.3} miles  ({:.0} m)", t.length / METRES_PER_MILE, t.length));
            ui.end_row();
            ui.label("Pieces");
            ui.label(format!("{}  ({} straight, {} curved)", t.segments.len(), t.segments.len() - curves, curves));
            ui.end_row();
            ui.label("Objects");
            ui.label(format!("{} placed", t.objects.len()));
            ui.end_row();
        })
        .response
        .on_hover_text(&l.source);

        // Lighting: the first thing that can be changed and saved.
        let night_now = t.night;
        let mut night = night_now;
        ui.horizontal(|ui| {
            ui.label("Lighting");
            ui.radio_value(&mut night, false, "Day");
            ui.radio_value(&mut night, true, "Night");
        });
        if night != night_now {
            self.set_night(night);
        }
        ui.add_space(2.0);
        self.save_panel(ui, ctx, settings);

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (v, label) in [(View::Map, "Map (from above)"), (View::ThreeD, "3D")] {
                let b = egui::Button::new(RichText::new(label).size(15.0).strong()).selected(self.view == v);
                if ui.add(b).clicked() {
                    self.view = v;
                }
            }
            if ui.button("Fit to screen").clicked() {
                self.fit();
            }
        });
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= ui.checkbox(&mut self.show_walls, "Show walls").changed();
            changed |= ui.checkbox(&mut self.show_objects, "Show objects").changed();
        });
        if changed {
            self.rebuild_mesh();
        }

        // Details of the selected piece
        if let Some(k) = self.selected {
            if let Some(l) = &self.loaded {
                ui.separator();
                piece_details(ui, &l.track.segments[k]);
            }
        }

        ui.separator();
        ui.label(RichText::new("Pieces of track").strong());
        ui.label(RichText::new("Click one to find it on the map.").weak());
        let mut clicked = None;
        ui.label(RichText::new("  #  Piece         Length  Radius  Bank").monospace().weak());
        if let Some(l) = &self.loaded {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for s in &l.track.segments {
                    let text = piece_line(s);
                    let r = ui.selectable_label(self.selected == Some(s.index), RichText::new(text).monospace());
                    if r.clicked() {
                        clicked = Some(s.index);
                    }
                    if self.selected == Some(s.index) && self.scroll_list {
                        r.scroll_to_me(Some(egui::Align::Center));
                    }
                }
            });
        }
        self.scroll_list = false;
        if let Some(k) = clicked {
            self.selected = Some(k);
            self.focus_segment(k);
        }
    }

    fn viewport(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let sky = if self.view == View::Map { Color32::from_rgb(28, 32, 36) } else { Color32::from_rgb(120, 160, 205) };
        painter.rect_filled(rect, 0.0, sky);
        self.aspect = (rect.width() / rect.height().max(1.0)) as f64;
        if self.fit_pending && self.loaded.is_some() {
            self.fit_pending = false;
            self.fit();
            let n = self.loaded.as_ref().map_or(0, |l| l.track.segments.len());
            if let Some(k) = self.select_on_load.take().filter(|&k| k < n) {
                self.selected = Some(k);
                self.scroll_list = true;
                self.focus_segment(k);
            }
        }
        let Some(l) = &self.loaded else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                if self.loading.is_some() { "Reading the track…" } else { "Pick a track on the left" },
                egui::FontId::proportional(20.0),
                Color32::from_gray(200),
            );
            return;
        };
        let aspect = self.aspect;

        // ---- input
        let scroll = if resp.hovered() { ui.input(|i| i.smooth_scroll_delta.y) as f64 } else { 0.0 };
        let delta = resp.drag_delta();
        let (dx, dy) = (delta.x as f64, delta.y as f64);
        match self.view {
            View::Map => {
                let mpp = 2.0 * self.cam.half_h / rect.height() as f64; // metres per pixel
                if resp.dragged() {
                    self.cam.cx -= dx * mpp;
                    self.cam.cy += dy * mpp;
                }
                if scroll != 0.0 {
                    // zoom about the cursor
                    let f = (-scroll * 0.0015).exp();
                    if let Some(p) = resp.hover_pos() {
                        let wx = self.cam.cx + (p.x - rect.center().x) as f64 * mpp;
                        let wy = self.cam.cy - (p.y - rect.center().y) as f64 * mpp;
                        self.cam.cx = wx + (self.cam.cx - wx) * f;
                        self.cam.cy = wy + (self.cam.cy - wy) * f;
                    }
                    self.cam.half_h = (self.cam.half_h * f).clamp(2.0, 50_000.0);
                }
                if resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let wx = self.cam.cx + (p.x - rect.center().x) as f64 * mpp + l.mesh.origin[0];
                        let wy = self.cam.cy - (p.y - rect.center().y) as f64 * mpp + l.mesh.origin[1];
                        self.selected = pick(&l.mesh, &l.track, wx, wy);
                        self.scroll_list = true;
                    }
                }
            }
            View::ThreeD => {
                if resp.dragged_by(egui::PointerButton::Primary) {
                    self.cam.yaw -= dx * 0.008;
                    self.cam.pitch = (self.cam.pitch + dy * 0.006).clamp(0.05, 1.55);
                }
                if resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle) {
                    let s = self.cam.dist * 0.0015;
                    let (sy, cy) = self.cam.yaw.sin_cos();
                    // Grab-the-ground panning. Screen right on the ground = (-sin yaw, cos yaw),
                    // forward (screen up) = (-cos yaw, -sin yaw).
                    self.cam.target[0] += (sy * dx - cy * dy) * s;
                    self.cam.target[1] += (-cy * dx - sy * dy) * s;
                }
                if scroll != 0.0 {
                    self.cam.dist = (self.cam.dist * (-scroll * 0.0015).exp()).clamp(5.0, 50_000.0);
                }
            }
        }

        // ---- draw
        let mvp = match self.view {
            View::Map => map_matrix(&self.cam, aspect, &l.mesh),
            View::ThreeD => orbit_matrix(&self.cam, aspect, &l.mesh),
        };
        let sel = self.selected.map(|k| k as f32).unwrap_or(-10.0);
        let scene = self.scene.clone();
        let cb = egui_glow::CallbackFn::new(move |_info, p| {
            scene.lock().unwrap().paint(p.gl(), &mvp, sel);
        });
        painter.add(egui::PaintCallback { rect, callback: Arc::new(cb) });

        // Labels on the map: start/finish and piece numbers when there's room.
        let to_screen = |x: f64, y: f64, z: f64| -> Option<egui::Pos2> {
            let o = l.mesh.origin;
            let v = [(x - o[0]) as f32, (y - o[1]) as f32, (z - o[2]) as f32, 1.0];
            let c = mul_vec(&mvp, v);
            if c[3] <= 0.0 {
                return None;
            }
            let (nx, ny) = (c[0] / c[3], c[1] / c[3]);
            Some(egui::pos2(
                rect.left() + (nx + 1.0) * 0.5 * rect.width(),
                rect.top() + (1.0 - ny) * 0.5 * rect.height(),
            ))
        };
        let font = egui::FontId::proportional(13.0);
        if let Some(s0) = l.track.segments.first() {
            let p = s0.point(0.0, 0.0);
            if let Some(q) = to_screen(p[0], p[1], s0.height(0.0, 0.0)) {
                painter.circle_filled(q, 4.0, Color32::WHITE);
                painter.text(q - egui::vec2(0.0, 6.0), egui::Align2::CENTER_BOTTOM, "Start / finish", font.clone(), Color32::WHITE);
            }
        }
        if self.view == View::Map {
            let mpp = 2.0 * self.cam.half_h / rect.height() as f64;
            for s in &l.track.segments {
                if s.length / mpp < 45.0 && self.selected != Some(s.index) {
                    continue;
                }
                let p = s.point(0.5, 0.0);
                if let Some(q) = to_screen(p[0], p[1], 0.0) {
                    let col = if self.selected == Some(s.index) { Color32::from_rgb(255, 170, 60) } else { Color32::from_gray(235) };
                    painter.text(q, egui::Align2::CENTER_CENTER, format!("{}", s.index + 1), font.clone(), col);
                }
            }
        }
        let hint = match self.view {
            View::Map => "Drag to move  ·  Scroll to zoom  ·  Click a piece of track to select it",
            View::ThreeD => "Drag to turn  ·  Right-drag to move  ·  Scroll to zoom",
        };
        painter.text(rect.left_top() + egui::vec2(10.0, 8.0), egui::Align2::LEFT_TOP, hint, font, Color32::from_gray(230));
    }
}

fn piece_line(s: &model::Segment) -> String {
    if s.curve {
        let dir = if s.angle > 0.0 { "Left " } else { "Right" };
        format!(
            "{:>3}  {} {:>5.1}°  {:>6.1}  {:>5.0}  {:>4.1}°",
            s.index + 1,
            dir,
            s.angle.abs().to_degrees(),
            s.length,
            s.radius,
            s.banking(0.5).abs()
        )
    } else {
        format!("{:>3}  Straight      {:>6.1}         {:>4.1}°", s.index + 1, s.length, s.banking(0.5).abs())
    }
}

fn piece_details(ui: &mut egui::Ui, s: &model::Segment) {
    let title = if s.curve {
        format!("Piece {} — {} turn", s.index + 1, if s.angle > 0.0 { "left" } else { "right" })
    } else {
        format!("Piece {} — straight", s.index + 1)
    };
    ui.label(RichText::new(title).strong().size(15.0));
    egui::Grid::new("piece_facts").num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
        ui.label("Length");
        ui.label(format!("{:.2} m  ({:.1} ft)", s.length, s.length * FEET_PER_METRE));
        ui.end_row();
        if s.curve {
            ui.label("Turns");
            ui.label(format!("{:.2}°", s.angle.abs().to_degrees()));
            ui.end_row();
            ui.label("Radius");
            ui.label(format!("{:.1} m  ({:.0} ft)", s.radius, s.radius * FEET_PER_METRE));
            ui.end_row();
        }
        ui.label("Banking");
        let (b0, b1) = (s.banking(0.0), s.banking(1.0));
        ui.label(if (b0 - b1).abs() < 0.05 {
            format!("{:.1}°", b0.abs())
        } else {
            format!("{:.1}° to {:.1}°", b0.abs(), b1.abs())
        });
        ui.end_row();
        let h0 = s.height(0.0, 0.0);
        let h1 = s.height(1.0, 0.0);
        ui.label("Height");
        ui.label(if (h0 - h1).abs() < 0.01 { format!("{h0:.2} m") } else { format!("{h0:.2} m to {h1:.2} m") });
        ui.end_row();
    });
    // Ground from right to left, as a driver sees it
    let strips = s.strips(0.5);
    if !strips.is_empty() {
        ui.label(RichText::new("Ground, right to left").weak());
        let mut line = String::new();
        let mut merged: Vec<(i64, f64)> = Vec::new();
        for (a, b, surf) in strips {
            match merged.last_mut() {
                Some(last) if last.0 == surf => last.1 += b - a,
                _ => merged.push((surf, b - a)),
            }
        }
        for (k, (surf, w)) in merged.iter().enumerate() {
            if k > 0 {
                line += ",  ";
            }
            line += &format!("{} {:.1} m", surface_name(*surf), w);
        }
        ui.label(line);
    }
    if !s.ws.is_empty() {
        ui.label(RichText::new("Walls").weak());
        let mut ws: Vec<_> = s.ws.iter().collect();
        ws.sort_by(|a, b| a.dlat[0].total_cmp(&b.dlat[0]));
        for w in ws {
            let d = 0.5 * (w.dlat[0] + w.dlat[1]);
            let side = if d < 0.0 { "right" } else { "left" };
            ui.label(format!(
                "{}  —  {:.1} m to the {side}, {:.2} m tall",
                wall_name(w.wall_type),
                d.abs(),
                0.5 * (w.height[0] + w.height[1])
            ))
            .on_hover_text(format!("Sandbox wall type {}", w.wall_type));
        }
    }
}

/// Nearest piece of track to a world point, if the point is on or near the track.
fn pick(m: &TrackMesh, t: &Track, x: f64, y: f64) -> Option<usize> {
    let mut best: Option<(f64, usize)> = None;
    for (k, line) in m.centerlines.iter().enumerate() {
        for w in line.windows(2) {
            let d = dist_to_segment([x, y], w[0], w[1]);
            if best.map_or(true, |b| d < b.0) {
                best = Some((d, k));
            }
        }
    }
    let (d, k) = best?;
    let s = &t.segments[k];
    let reach = (s.max_dlat(0.5) - s.min_dlat(0.5)).abs().max(30.0);
    (d <= reach).then_some(k)
}

fn dist_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 > 0.0 { (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (p[0] - a[0] - t * ab[0]).hypot(p[1] - a[1] - t * ab[1])
}

// ------------------------------------------------------------------ matrices (column-major)
type Mat = [f32; 16];

fn mat_mul(a: &Mat, b: &Mat) -> Mat {
    let mut r = [0.0f32; 16];
    for c in 0..4 {
        for row in 0..4 {
            r[c * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[c * 4 + k]).sum();
        }
    }
    r
}

fn mul_vec(m: &Mat, v: [f32; 4]) -> [f32; 4] {
    let mut r = [0.0f32; 4];
    for row in 0..4 {
        r[row] = (0..4).map(|k| m[k * 4 + row] * v[k]).sum();
    }
    r
}

fn map_matrix(c: &Camera, aspect: f64, m: &TrackMesh) -> Mat {
    let hh = c.half_h;
    let hw = hh * aspect;
    let zr = (m.extent[3] - m.extent[2]).max(1.0) * 4.0 + 100.0;
    // x,y -> ndc; higher z is nearer (smaller depth)
    [
        (1.0 / hw) as f32, 0.0, 0.0, 0.0,
        0.0, (1.0 / hh) as f32, 0.0, 0.0,
        0.0, 0.0, (-1.0 / zr) as f32, 0.0,
        (-c.cx / hw) as f32, (-c.cy / hh) as f32, 0.0, 1.0,
    ]
}

fn orbit_matrix(c: &Camera, aspect: f64, m: &TrackMesh) -> Mat {
    let (sp, cp) = c.pitch.sin_cos();
    let (sy, cy) = c.yaw.sin_cos();
    let eye = [c.target[0] + c.dist * cp * cy, c.target[1] + c.dist * cp * sy, c.target[2] + c.dist * sp];
    // look-at with z up
    let f = norm([c.target[0] - eye[0], c.target[1] - eye[1], c.target[2] - eye[2]]);
    let s = norm(cross(f, [0.0, 0.0, 1.0]));
    let u = cross(s, f);
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let view: Mat = [
        s[0] as f32, u[0] as f32, -f[0] as f32, 0.0,
        s[1] as f32, u[1] as f32, -f[1] as f32, 0.0,
        s[2] as f32, u[2] as f32, -f[2] as f32, 0.0,
        -dot(s, eye) as f32, -dot(u, eye) as f32, dot(f, eye) as f32, 1.0,
    ];
    let size = m.extent[0].max(m.extent[1]).max(10.0);
    let near = (c.dist * 0.01).max(0.2);
    let far = c.dist * 4.0 + size * 2.0;
    let fov: f64 = 50f64.to_radians();
    let t = 1.0 / (fov / 2.0).tan();
    let proj: Mat = [
        (t / aspect) as f32, 0.0, 0.0, 0.0,
        0.0, t as f32, 0.0, 0.0,
        0.0, 0.0, ((far + near) / (near - far)) as f32, -1.0,
        0.0, 0.0, ((2.0 * far * near) / (near - far)) as f32, 0.0,
    ];
    mat_mul(&proj, &view)
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn norm(a: [f64; 3]) -> [f64; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-12);
    [a[0] / l, a[1] / l, a[2] / l]
}

// ------------------------------------------------------------------ OpenGL
#[derive(Default)]
struct Scene {
    gl: Option<GlObjects>,
    failed: bool,
    pending: Option<Vec<f32>>,
    count: i32,
}

struct GlObjects {
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
}

const VS: &str = r#"
IN vec3 a_pos;
IN vec3 a_col;
IN vec3 a_nrm;
IN float a_seg;
uniform mat4 u_mvp;
uniform float u_sel;
OUT vec3 v_col;
void main() {
    vec3 light = normalize(vec3(0.35, 0.25, 0.9));
    float lit = 0.55 + 0.45 * abs(dot(normalize(a_nrm), light));
    vec3 c = a_col;
    if (abs(a_seg - u_sel) < 0.5) c = mix(c, vec3(1.0, 0.6, 0.15), 0.55);
    v_col = c * lit;
    gl_Position = u_mvp * vec4(a_pos, 1.0);
}
"#;

const FS: &str = r#"
IN vec3 v_col;
void main() {
    FRAG = vec4(v_col, 1.0);
}
"#;

impl Scene {
    fn init(gl: &glow::Context) -> Result<GlObjects, String> {
        let sv = egui_glow::ShaderVersion::get(gl);
        let new = sv.is_new_shader_interface();
        let head = format!(
            "{}\n#ifdef GL_ES\nprecision highp float;\n#endif\n",
            sv.version_declaration()
        );
        let vs = format!(
            "{head}{}\n{VS}",
            if new { "#define IN in\n#define OUT out" } else { "#define IN attribute\n#define OUT varying" }
        );
        let fs = format!(
            "{head}{}\n{FS}",
            if new { "#define IN in\nout vec4 o_color;\n#define FRAG o_color" } else { "#define IN varying\n#define FRAG gl_FragColor" }
        );
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            for (kind, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
                let sh = gl.create_shader(kind)?;
                gl.shader_source(sh, &src);
                gl.compile_shader(sh);
                if !gl.get_shader_compile_status(sh) {
                    return Err(format!("3D view shader: {}", gl.get_shader_info_log(sh)));
                }
                gl.attach_shader(program, sh);
                shaders.push(sh);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(format!("3D view program: {}", gl.get_program_info_log(program)));
            }
            for sh in shaders {
                gl.detach_shader(program, sh);
                gl.delete_shader(sh);
            }
            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let stride = (VERTEX_FLOATS * 4) as i32;
            for (name, size, offset) in [("a_pos", 3, 0), ("a_col", 3, 3), ("a_nrm", 3, 6), ("a_seg", 1, 9)] {
                if let Some(loc) = gl.get_attrib_location(program, name) {
                    gl.enable_vertex_attrib_array(loc);
                    gl.vertex_attrib_pointer_f32(loc, size, glow::FLOAT, false, stride, offset * 4);
                }
            }
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            Ok(GlObjects { program, vao, vbo })
        }
    }

    fn paint(&mut self, gl: &glow::Context, mvp: &Mat, sel: f32) {
        if self.failed {
            return;
        }
        if self.gl.is_none() {
            match Self::init(gl) {
                Ok(o) => self.gl = Some(o),
                Err(e) => {
                    eprintln!("{e}");
                    self.failed = true;
                    return;
                }
            }
        }
        let o = self.gl.as_ref().unwrap();
        unsafe {
            if let Some(v) = self.pending.take() {
                let bytes = std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 4);
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(o.vbo));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
                gl.bind_buffer(glow::ARRAY_BUFFER, None);
                self.count = (v.len() / VERTEX_FLOATS) as i32;
            }
            if self.count == 0 {
                return;
            }
            gl.use_program(Some(o.program));
            gl.uniform_matrix_4_f32_slice(gl.get_uniform_location(o.program, "u_mvp").as_ref(), false, mvp);
            gl.uniform_1_f32(gl.get_uniform_location(o.program, "u_sel").as_ref(), sel);
            gl.bind_vertex_array(Some(o.vao));
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.draw_arrays(glow::TRIANGLES, 0, self.count);
            gl.disable(glow::DEPTH_TEST);
            gl.enable(glow::BLEND);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }
}
