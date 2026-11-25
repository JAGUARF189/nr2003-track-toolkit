//! "Track" tab: open a track (.ptf, or the one inside a .dat) and look at it, from above or
//! in 3D, with every piece of track listed in plain numbers; edit its shape (editor.rs) and
//! its placed objects (objects_ui.rs); save safely (saving.rs).

use eframe::egui::{self, Color32, RichText, Sense};
use eframe::egui_glow;
use eframe::glow::{self, HasContext};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use ttk::config::Settings;

mod editor;
mod objects_ui;
mod parts;
mod saving;
#[cfg(test)]
mod ui_tests;

use ttk::track::check::Problem;
use ttk::track::mesh::{self, Batch, TrackMesh, VERTEX_FLOATS};
use ttk::track::textures::TexStore;

/// Textures are loaded at about this many pixels on their longer side.
const TEX_SIDE: u32 = 256;
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

/// What clicking on the map works on.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Pieces,
    Objects,
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
    /// Textures/objects the track uses that can't be found (checked once when loading).
    file_problems: Vec<Problem>,
    /// The track's folder (where its textures and objects are): where added objects are copied.
    folder: Option<PathBuf>,
    /// Finds the track's .mip textures.
    tex_store: Option<Arc<TexStore>>,
    /// Textures decoded while loading: (lower-case file, picture), and every file tried.
    tex_images: Vec<(String, image::RgbaImage)>,
    tex_tried: Vec<String>,
}

pub(crate) struct Camera {
    // map (top-down): centre and half height of the visible area, in mesh metres
    pub cx: f64,
    pub cy: f64,
    pub half_h: f64,
    // 3D orbit
    pub target: [f64; 3],
    pub yaw: f64,
    pub pitch: f64,
    pub dist: f64,
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
    /// Draw the track's real textures (else plain colours by surface and wall type).
    show_textures: bool,
    /// Textures already sent to the 3D view (or tried and not found), lower-case.
    tex_loaded: HashSet<String>,
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
    /// Checker findings for the current track.
    problems: Vec<Problem>,
    move_objects: bool,
    edit_error: Option<String>,
    /// The shape changed since loading (AI lines, cameras and stalls need redoing).
    layout_changed: bool,
    mode: Mode,
    /// The wall or ground strip picked on the selected piece (graph id).
    part: Option<i32>,
    /// "Copy to pieces A to B" (1-based piece numbers).
    copy_to: (usize, usize),
    /// The part `copy_to` was suggested for.
    copy_for: Option<i32>,
    part_note: Option<String>,
    /// Papyrus's tools (makemip for pictures made into textures), from Settings.
    sandbox_dir: PathBuf,
    /// Scroll the side panel to the wall/strip editor (after picking one on the map).
    scroll_part: bool,
    /// Wall (true) or strip number to pick once loaded (command line).
    part_on_load: Option<(bool, usize)>,
    cam_on_load: (Option<f64>, Option<f64>, Option<f64>),
    /// Selected placed objects (graph ids), in the order picked.
    obj_sel: Vec<i32>,
    /// Locked objects can't be picked on the map.
    locked: HashSet<i32>,
    obj_drag: Option<objects_ui::Drag>,
    obj_filter: String,
    /// Last click on the map in Objects mode, to step through objects on top of each other.
    obj_cycle: Option<(egui::Pos2, Vec<i32>, usize)>,
    obj_note: Option<String>,
    /// Scroll the object list to the selection on the next frame.
    obj_scroll: bool,
    /// Counter that makes each drag on the map its own Undo step.
    obj_key: u64,
    /// The object markers need redrawing (moved, selected, locked...).
    objects_dirty: bool,
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
            show_textures: true,
            tex_loaded: HashSet::new(),
            scroll_list: false,
            aspect: 1.4,
            select_on_load: None,
            fit_pending: false,
            ed: Default::default(),
            workspace: None,
            problems: Vec::new(),
            move_objects: true,
            edit_error: None,
            layout_changed: false,
            mode: Mode::Pieces,
            part: None,
            copy_to: (1, 1),
            copy_for: None,
            part_note: None,
            sandbox_dir: PathBuf::new(),
            scroll_part: false,
            part_on_load: None,
            cam_on_load: (None, None, None),
            obj_sel: Vec::new(),
            locked: HashSet::new(),
            obj_drag: None,
            obj_filter: String::new(),
            obj_cycle: None,
            obj_note: None,
            obj_scroll: false,
            obj_key: 0,
            objects_dirty: true,
            scene: Arc::new(Mutex::new(Scene::default())),
        }
    }
}

impl TrackView {
    /// Load a track in the background. `restore_to`: `path` is an earlier version of that file.
    fn open(&mut self, ctx: &egui::Context, name: String, path: PathBuf, restore_to: Option<PathBuf>) {
        let (tx, rx) = mpsc::channel();
        let walls = self.show_walls;
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
                let mesh = mesh::build(&track, walls);
                let restored = restore_to.is_some();
                let save_path = restore_to.or_else(|| {
                    (ttk::util::ext_lower(&path) == "ptf").then(|| path.clone())
                });
                if restored {
                    source = format!("earlier version: {source}");
                }
                // Look for the textures and objects next to the real track file.
                let folder = save_path.as_ref().or(Some(&path)).and_then(|p| p.parent()).map(Path::to_path_buf);
                let file_problems = folder
                    .as_deref()
                    .map(|dir| ttk::track::check::missing_files(&g, dir, shared_folder(dir).as_deref()))
                    .unwrap_or_default();
                // Decode the textures here, off the screen thread.
                let tex_store = folder.as_deref().map(|d| Arc::new(TexStore::new(d)));
                let tex_tried: Vec<String> = mesh.textures();
                let tex_images = match &tex_store {
                    Some(st) => tex_tried.iter().filter_map(|f| st.load(f, TEX_SIDE).map(|i| (f.to_ascii_lowercase(), i))).collect(),
                    None => Vec::new(),
                };
                Ok(Loaded {
                    name,
                    source,
                    graph: g,
                    track,
                    mesh,
                    save_path,
                    restored,
                    file_problems,
                    folder,
                    tex_store,
                    tex_images,
                    tex_tried,
                })
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

    /// Pick wall (or ground strip) number `n`, right to left, of the piece chosen with `--piece`.
    pub fn pick_on_load(&mut self, wall: bool, n: usize) {
        self.part_on_load = Some((wall, n));
    }

    /// 3D camera distance (metres) and angles (degrees) once loaded (command line).
    pub fn camera_on_load(&mut self, dist: Option<f64>, yaw: Option<f64>, pitch: Option<f64>) {
        self.cam_on_load = (dist, yaw, pitch);
    }

    /// Open a track file, asking first if the current one has unsaved changes (another tab
    /// sending a track here).
    pub fn open_file(&mut self, ctx: &egui::Context, path: PathBuf) {
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        self.request_open(ctx, name, path);
    }

    /// Start in Objects mode (command line `--objects`).
    pub fn objects_mode(&mut self) {
        self.mode = Mode::Objects;
    }

    fn poll(&mut self) {
        let Some(rx) = &self.loading else { return };
        match rx.try_recv() {
            Ok(Ok(mut l)) => {
                {
                    let mut scene = self.scene.lock().unwrap();
                    scene.clear_textures = true;
                    scene.pending_tex = std::mem::take(&mut l.tex_images);
                }
                self.tex_loaded = l.tex_tried.iter().map(|f| f.to_ascii_lowercase()).collect();
                self.selected = None;
                let restored = l.restored;
                self.locked = l.save_path.as_deref().map(|p| ttk::track::objects::load_locks(p, &l.track)).unwrap_or_default();
                self.obj_sel.clear();
                self.part = None;
                self.part_note = None;
                self.obj_drag = None;
                self.obj_cycle = None;
                self.obj_note = None;
                self.objects_dirty = true;
                self.loaded = Some(l);
                self.upload_mesh();
                self.reset_edits(restored);
                self.layout_changed = false;
                self.edit_error = None;
                self.recheck();
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
            l.mesh = mesh::build(&l.track, self.show_walls);
        }
        self.upload_mesh();
        self.objects_dirty = true;
    }

    /// Send the track mesh to the 3D view, loading any textures it needs that aren't there yet
    /// (a texture picked in the wall/strip editor, say).
    pub(super) fn upload_mesh(&mut self) {
        let Some(l) = &mut self.loaded else { return };
        let mut scene = self.scene.lock().unwrap();
        scene.pending = Some((std::mem::take(&mut l.mesh.verts), std::mem::take(&mut l.mesh.indices), l.mesh.batches.clone()));
        if let Some(store) = &l.tex_store {
            for f in l.mesh.textures() {
                let key = f.to_ascii_lowercase();
                if self.tex_loaded.insert(key.clone()) {
                    if let Some(img) = store.load(&f, TEX_SIDE) {
                        scene.pending_tex.push((key, img));
                    }
                }
            }
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
        self.sandbox_dir = settings.sandbox_dir.clone();
        self.poll();
        self.workspace = Some(env.workspace.to_path_buf());
        if self.loaded.is_some() && self.ed.pending.is_none() {
            self.shortcuts(ctx);
            if self.mode == Mode::Objects {
                self.objects_keys(ctx);
            }
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
        self.problems_panel(ui);

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
            ui.checkbox(&mut self.show_textures, "Textures")
                .on_hover_text("Show the track's own pictures (.mip). Off: plain colours by surface and wall type.");
        });
        if changed {
            self.rebuild_mesh();
        }

        ui.horizontal(|ui| {
            ui.label("Work on");
            for (m, label) in [(Mode::Pieces, "Track pieces"), (Mode::Objects, "Objects")] {
                let b = egui::Button::new(RichText::new(label).size(15.0).strong()).selected(self.mode == m);
                if ui.add(b).clicked() && self.mode != m {
                    self.mode = m;
                    self.obj_drag = None;
                    self.objects_dirty = true;
                }
            }
        });
        if self.mode == Mode::Objects {
            ui.separator();
            self.objects_panel(ui);
            return;
        }

        // The selected piece: edit its shape and banking
        if let Some(k) = self.selected {
            if self.loaded.as_ref().is_some_and(|l| k < l.track.segments.len()) {
                ui.separator();
                // Its own scroll area, so the list of pieces below stays in view.
                let h = (ui.available_height() * 0.62).max(160.0);
                egui::ScrollArea::vertical().id_salt("piece_edit_scroll").max_height(h).auto_shrink([false, true]).show(ui, |ui| {
                    self.piece_editor(ui, k);
                });
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
                    let (bad, warned) = editor::marks(&self.problems, s.index);
                    let mut rt = RichText::new(text).monospace();
                    if bad {
                        rt = rt.color(Color32::from_rgb(235, 95, 85));
                    } else if warned {
                        rt = rt.color(Color32::from_rgb(240, 190, 80));
                    }
                    let r = ui.selectable_label(self.selected == Some(s.index), rt);
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
            if self.selected != Some(k) {
                self.part = None;
                self.part_note = None;
            }
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
            let (d, y, p) = std::mem::take(&mut self.cam_on_load);
            if let Some(d) = d {
                self.cam.dist = d.max(5.0);
            }
            if let Some(y) = y {
                self.cam.yaw = y.to_radians();
            }
            if let Some(p) = p {
                self.cam.pitch = p.to_radians().clamp(0.05, 1.55);
            }
            let n = self.loaded.as_ref().map_or(0, |l| l.track.segments.len());
            if let Some(k) = self.select_on_load.take().filter(|&k| k < n) {
                self.selected = Some(k);
                self.scroll_list = true;
                self.focus_segment(k);
                if let (Some((wall, i)), Some(l)) = (self.part_on_load.take(), &self.loaded) {
                    let s = &l.track.segments[k];
                    let mut ids: Vec<(f64, i32)> = if wall {
                        s.ws.iter().map(|w| (w.dlat[0] + w.dlat[1], w.id)).collect()
                    } else {
                        s.fs.iter().map(|f| (f.dlat[0] + f.dlat[1], f.id)).collect()
                    };
                    ids.sort_by(|a, b| a.0.total_cmp(&b.0));
                    self.part = i.checked_sub(1).and_then(|i| ids.get(i)).map(|x| x.1);
                    self.scroll_part = self.part.is_some();
                }
            }
        }
        if self.mode == Mode::Objects && self.view == View::Map {
            self.objects_map_input(ui, rect, &resp);
        }
        if self.objects_dirty {
            self.objects_dirty = false;
            let verts = self.object_verts();
            self.scene.lock().unwrap().pending_objects = Some(verts);
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
        let left_pans = self.mode == Mode::Pieces;
        camera_input(&mut self.cam, self.view == View::ThreeD, ui, &resp, rect, left_pans);
        if self.view == View::Map && resp.clicked() && self.mode == Mode::Pieces {
            if let Some(p) = resp.interact_pointer_pos() {
                let mpp = 2.0 * self.cam.half_h / rect.height() as f64;
                let wx = self.cam.cx + (p.x - rect.center().x) as f64 * mpp + l.mesh.origin[0];
                let wy = self.cam.cy - (p.y - rect.center().y) as f64 * mpp + l.mesh.origin[1];
                let before = self.selected;
                self.selected = pick(&l.mesh, &l.track, wx, wy);
                let picked = self.selected.and_then(|k| parts::pick_part(&l.track.segments[k], wx, wy, (6.0 * mpp).max(0.3)));
                if picked.is_some() || self.selected != before {
                    self.part = picked;
                    self.part_note = None;
                    self.scroll_part = picked.is_some();
                }
                self.scroll_list = true;
            }
        }

        // ---- draw
        let mvp = match self.view {
            View::Map => map_matrix(&self.cam, aspect, &l.mesh),
            View::ThreeD => orbit_matrix(&self.cam, aspect, &l.mesh),
        };
        let sel = self.selected.map(|k| k as f32).unwrap_or(-10.0);
        let scene = self.scene.clone();
        let textured = self.show_textures;
        let cb = egui_glow::CallbackFn::new(move |_info, p| {
            scene.lock().unwrap().paint(p.gl(), &mvp, sel, textured);
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
        if let (Mode::Pieces, Some(k), Some(id)) = (self.mode, self.selected, self.part) {
            if let Some(s) = l.track.segments.get(k) {
                let pts: Vec<egui::Pos2> = parts::part_outline(s, id).iter().filter_map(|p| to_screen(p[0], p[1], p[2])).collect();
                if pts.len() > 2 {
                    painter.add(egui::Shape::closed_line(pts, egui::Stroke::new(2.5f32, Color32::from_rgb(80, 200, 255))));
                }
            }
        }
        if self.view == View::Map && self.show_objects {
            self.objects_overlay(&painter, rect);
        }
        let hint = match (self.view, self.mode) {
            (View::Map, Mode::Pieces) => "Drag to move  ·  Scroll to zoom  ·  Click the track to pick a piece and the wall or ground there",
            (View::Map, Mode::Objects) => {
                "Click an object to select it (click again for the one under it)  ·  Ctrl+click adds  ·  Drag a box around several\n\
                 Drag them to move  ·  Drag the round handle to turn  ·  Right-drag to move the map  ·  Scroll to zoom"
            }
            (View::ThreeD, Mode::Pieces) => "Drag to turn  ·  Right-drag to move  ·  Scroll to zoom",
            (View::ThreeD, Mode::Objects) => "Drag to turn  ·  Right-drag to move  ·  Scroll to zoom  ·  Pick objects on the Map view",
        };
        painter.text(rect.left_top() + egui::vec2(10.0, 8.0), egui::Align2::LEFT_TOP, hint, font, Color32::from_gray(230));
    }
}

/// The game's `tracks\shared` folder next to a track folder, if there is one.
fn shared_folder(track_dir: &Path) -> Option<PathBuf> {
    track_dir.parent().map(|t| t.join("shared")).filter(|d| d.is_dir())
}

/// Moving the camera with the mouse: on the map, drag (left if `left_pans`, else right or
/// middle) to move and scroll to zoom about the cursor; in 3D, drag to turn, right/middle-drag
/// to move, scroll to zoom.
pub(crate) fn camera_input(cam: &mut Camera, three_d: bool, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect, left_pans: bool) {
    let scroll = if resp.hovered() { ui.input(|i| i.smooth_scroll_delta.y) as f64 } else { 0.0 };
    let delta = resp.drag_delta();
    let (dx, dy) = (delta.x as f64, delta.y as f64);
    if !three_d {
        let mpp = 2.0 * cam.half_h / rect.height() as f64; // metres per pixel
        let pan = if left_pans {
            resp.dragged()
        } else {
            resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle)
        };
        if pan {
            cam.cx -= dx * mpp;
            cam.cy += dy * mpp;
        }
        if scroll != 0.0 {
            // zoom about the cursor
            let f = (-scroll * 0.0015).exp();
            if let Some(p) = resp.hover_pos() {
                let wx = cam.cx + (p.x - rect.center().x) as f64 * mpp;
                let wy = cam.cy - (p.y - rect.center().y) as f64 * mpp;
                cam.cx = wx + (cam.cx - wx) * f;
                cam.cy = wy + (cam.cy - wy) * f;
            }
            cam.half_h = (cam.half_h * f).clamp(2.0, 50_000.0);
        }
    } else {
        if resp.dragged_by(egui::PointerButton::Primary) {
            cam.yaw -= dx * 0.008;
            cam.pitch = (cam.pitch + dy * 0.006).clamp(0.05, 1.55);
        }
        if resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle) {
            let s = cam.dist * 0.0015;
            let (sy, cy) = cam.yaw.sin_cos();
            // Grab-the-ground panning. Screen right on the ground = (-sin yaw, cos yaw),
            // forward (screen up) = (-cos yaw, -sin yaw).
            cam.target[0] += (sy * dx - cy * dy) * s;
            cam.target[1] += (-cy * dx - sy * dy) * s;
        }
        if scroll != 0.0 {
            cam.dist = (cam.dist * (-scroll * 0.0015).exp()).clamp(5.0, 50_000.0);
        }
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
pub(crate) type Mat = [f32; 16];

fn mat_mul(a: &Mat, b: &Mat) -> Mat {
    let mut r = [0.0f32; 16];
    for c in 0..4 {
        for row in 0..4 {
            r[c * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[c * 4 + k]).sum();
        }
    }
    r
}

pub(crate) fn mul_vec(m: &Mat, v: [f32; 4]) -> [f32; 4] {
    let mut r = [0.0f32; 4];
    for row in 0..4 {
        r[row] = (0..4).map(|k| m[k * 4 + row] * v[k]).sum();
    }
    r
}

pub(crate) fn map_matrix(c: &Camera, aspect: f64, m: &TrackMesh) -> Mat {
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

pub(crate) fn orbit_matrix(c: &Camera, aspect: f64, m: &TrackMesh) -> Mat {
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
/// Two layers drawn with the same program: the track, and the object markers (rebuilt on
/// their own when objects move or the selection changes).
#[derive(Default)]
pub(crate) struct Scene {
    gl: Option<GlObjects>,
    failed: bool,
    /// Track layer to upload: vertices, indices (empty = not indexed), batches.
    pub pending: Option<(Vec<f32>, Vec<u32>, Vec<Batch>)>,
    pub pending_objects: Option<Vec<f32>>,
    /// Textures to upload: (lower-case file, picture).
    pub pending_tex: Vec<(String, image::RgbaImage)>,
    /// Delete the uploaded textures first (another track was opened).
    pub clear_textures: bool,
    batches: Vec<Batch>,
    /// The track layer is drawn from indices.
    indexed: bool,
    count_objects: i32,
    textures: std::collections::HashMap<String, glow::Texture>,
}

struct GlObjects {
    program: glow::Program,
    /// (vertex array, buffer) for the track, then the objects
    layers: [(glow::VertexArray, glow::Buffer); 2],
    /// Index buffer of the track layer (bound in its vertex array).
    ebo: glow::Buffer,
    /// 1x1 white picture for untextured batches.
    white: glow::Texture,
}

const VS: &str = r#"
IN vec3 a_pos;
IN vec3 a_col;
IN vec3 a_nrm;
IN float a_seg;
IN vec2 a_uv;
uniform mat4 u_mvp;
uniform float u_sel;
OUT vec3 v_col;
OUT vec2 v_uv;
OUT float v_lit;
OUT float v_sel;
void main() {
    vec3 light = normalize(vec3(0.35, 0.25, 0.9));
    v_lit = 0.55 + 0.45 * abs(dot(normalize(a_nrm), light));
    v_col = a_col;
    v_uv = a_uv;
    v_sel = abs(a_seg - u_sel) < 0.5 ? 1.0 : 0.0;
    gl_Position = u_mvp * vec4(a_pos, 1.0);
}
"#;

const FS: &str = r#"
IN vec3 v_col;
IN vec2 v_uv;
IN float v_lit;
IN float v_sel;
uniform sampler2D u_tex;
uniform float u_textured;
uniform float u_alpha;
void main() {
    vec4 t = TEX(u_tex, v_uv);
    if (u_alpha > 0.5 && t.a < 0.5) discard;
    vec3 c = mix(v_col, t.rgb, u_textured);
    c = mix(c, vec3(1.0, 0.6, 0.15), (0.55 - 0.2 * u_textured) * v_sel);
    FRAG = vec4(c * v_lit, 1.0);
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
            if new {
                "#define IN in\nout vec4 o_color;\n#define FRAG o_color\n#define TEX texture"
            } else {
                "#define IN varying\n#define FRAG gl_FragColor\n#define TEX texture2D"
            }
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
            let layer = || -> Result<(glow::VertexArray, glow::Buffer), String> {
                let vao = gl.create_vertex_array()?;
                let vbo = gl.create_buffer()?;
                gl.bind_vertex_array(Some(vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                let stride = (VERTEX_FLOATS * 4) as i32;
                for (name, size, offset) in [("a_pos", 3, 0), ("a_col", 3, 3), ("a_nrm", 3, 6), ("a_seg", 1, 9), ("a_uv", 2, 10)] {
                    if let Some(loc) = gl.get_attrib_location(program, name) {
                        gl.enable_vertex_attrib_array(loc);
                        gl.vertex_attrib_pointer_f32(loc, size, glow::FLOAT, false, stride, offset * 4);
                    }
                }
                gl.bind_vertex_array(None);
                gl.bind_buffer(glow::ARRAY_BUFFER, None);
                Ok((vao, vbo))
            };
            let layers = [layer()?, layer()?];
            let ebo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(layers[0].0));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
            let white = Self::upload_texture(gl, 1, 1, &[255, 255, 255, 255])?;
            Ok(GlObjects { program, layers, ebo, white })
        }
    }

    /// Upload an RGBA picture as a repeating, mipmapped texture.
    unsafe fn upload_texture(gl: &glow::Context, w: u32, h: u32, rgba: &[u8]) -> Result<glow::Texture, String> {
        let t = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(t));
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA as i32,
            w as i32,
            h as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(rgba)),
        );
        gl.generate_mipmap(glow::TEXTURE_2D);
        gl.bind_texture(glow::TEXTURE_2D, None);
        Ok(t)
    }

    pub fn paint(&mut self, gl: &glow::Context, mvp: &Mat, sel: f32, textured: bool) {
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
            if std::mem::take(&mut self.clear_textures) {
                for (_, t) in self.textures.drain() {
                    gl.delete_texture(t);
                }
            }
            for (name, img) in std::mem::take(&mut self.pending_tex) {
                match Self::upload_texture(gl, img.width(), img.height(), img.as_raw()) {
                    Ok(t) => {
                        if let Some(old) = self.textures.insert(name, t) {
                            gl.delete_texture(old);
                        }
                    }
                    Err(e) => eprintln!("texture {name}: {e}"),
                }
            }
            let track = self.pending.take().map(|(v, ix, b)| {
                self.batches = b;
                self.indexed = !ix.is_empty();
                if self.indexed {
                    let bytes = std::slice::from_raw_parts(ix.as_ptr() as *const u8, ix.len() * 4);
                    gl.bind_vertex_array(Some(o.layers[0].0));
                    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(o.ebo));
                    gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
                    gl.bind_vertex_array(None);
                }
                v
            });
            for (k, pending) in [track, self.pending_objects.take()].into_iter().enumerate() {
                if let Some(v) = pending {
                    let bytes = std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 4);
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(o.layers[k].1));
                    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::DYNAMIC_DRAW);
                    gl.bind_buffer(glow::ARRAY_BUFFER, None);
                    if k == 1 {
                        self.count_objects = (v.len() / VERTEX_FLOATS) as i32;
                    }
                }
            }
            if self.batches.is_empty() && self.count_objects == 0 {
                return;
            }
            gl.use_program(Some(o.program));
            gl.uniform_matrix_4_f32_slice(gl.get_uniform_location(o.program, "u_mvp").as_ref(), false, mvp);
            gl.uniform_1_f32(gl.get_uniform_location(o.program, "u_sel").as_ref(), sel);
            gl.uniform_1_i32(gl.get_uniform_location(o.program, "u_tex").as_ref(), 0);
            let u_textured = gl.get_uniform_location(o.program, "u_textured");
            let u_alpha = gl.get_uniform_location(o.program, "u_alpha");
            gl.active_texture(glow::TEXTURE0);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            // The track, one batch per texture (colour only when textures are off or missing).
            gl.bind_vertex_array(Some(o.layers[0].0));
            for b in &self.batches {
                let tex = if textured { b.texture.as_ref().and_then(|f| self.textures.get(&f.to_ascii_lowercase())) } else { None };
                gl.bind_texture(glow::TEXTURE_2D, Some(*tex.unwrap_or(&o.white)));
                gl.uniform_1_f32(u_textured.as_ref(), if tex.is_some() { 1.0 } else { 0.0 });
                gl.uniform_1_f32(u_alpha.as_ref(), if tex.is_some() && b.alpha_test { 1.0 } else { 0.0 });
                if self.indexed {
                    gl.draw_elements(glow::TRIANGLES, b.count, glow::UNSIGNED_INT, b.first * 4);
                } else {
                    gl.draw_arrays(glow::TRIANGLES, b.first, b.count);
                }
            }
            // The object markers: plain colours.
            if self.count_objects > 0 {
                gl.bind_vertex_array(Some(o.layers[1].0));
                gl.bind_texture(glow::TEXTURE_2D, Some(o.white));
                gl.uniform_1_f32(u_textured.as_ref(), 0.0);
                gl.uniform_1_f32(u_alpha.as_ref(), 0.0);
                gl.draw_arrays(glow::TRIANGLES, 0, self.count_objects);
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.disable(glow::DEPTH_TEST);
            gl.enable(glow::BLEND);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }
}
