//! "Assetto Corsa" tab: pick an AC track and layout and look at it, from above or in 3D, with
//! its textures, the AI line with the track edges, and the pit boxes and grid. The first step
//! of converting AC tracks to NR2003 (phase 1: reading and looking).

use crate::track_ui::{camera_input, map_matrix, mul_vec, orbit_matrix, Camera, Scene};
use eframe::egui::{self, Color32, RichText, Sense};
use eframe::egui_glow;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use ttk::ac::{self, AcInfo, AcScene, ViewOptions};
use ttk::{acconv, acscenery, acsupport};
use ttk::config::Settings;
use ttk::track::mesh::TrackMesh;

/// Textures are decoded at about this many pixels on their longer side.
const TEX_SIDE: u32 = 256;

struct Loaded {
    title: String,
    scene: AcScene,
    mesh: TrackMesh,
    summary: String,
    /// AC folder and layout it came from (for converting).
    dir: PathBuf,
    layout: String,
}

/// What became of AC's scenery.
struct SceneryMade {
    stats: acscenery::Stats,
    objects: usize,
    mips: usize,
    problems: Vec<String>,
}

/// How "build and install" went.
struct Installed {
    ok: bool,
    /// Where it went in NR2003 (on success), or what stopped it.
    message: String,
    /// The build's warnings (e.g. placeholder .lp files), plain text.
    warnings: Vec<String>,
}

/// A finished conversion.
struct Made {
    /// None when building and installing wasn't asked for.
    installed: Option<Installed>,
    report: acconv::Report,
    written: acconv::Written,
    /// None when scenery wasn't asked for.
    scenery: Option<SceneryMade>,
    /// Flaggers, track.ini and racing-line files (or why they couldn't be made).
    support: Result<acsupport::Support, String>,
    /// The NR2003 centerline in AC coordinates, to draw over the AC track.
    line: Vec<[f32; 3]>,
}

pub struct AcView {
    tracks_dir: Option<PathBuf>,
    tracks: Vec<(String, PathBuf)>,
    filter: String,
    picked: Option<usize>,
    info: Option<AcInfo>,
    layout: usize,
    loading: Option<mpsc::Receiver<Result<Loaded, String>>>,
    loaded: Option<Loaded>,
    error: Option<String>,
    three_d: bool,
    cam: Camera,
    opts: ViewOptions,
    show_ai: bool,
    show_markers: bool,
    show_textures: bool,
    aspect: f64,
    fit_pending: bool,
    scene: Arc<Mutex<Scene>>,
    /// Folder/track/layout given on the command line, opened on the first frame.
    open_on_start: Option<(PathBuf, String)>,
    /// Name of the NR2003 track to make.
    nr_name: String,
    making: Option<mpsc::Receiver<Result<Made, String>>>,
    made: Option<Made>,
    make_error: Option<String>,
    show_nr: bool,
    /// Convert with a looser fit (fewer pieces).
    fewer: bool,
    /// Bring AC's scenery as NR2003 objects, with textures up to `scenery_tex` pixels.
    scenery: bool,
    scenery_tex: u32,
    /// Then build the track and install it into NR2003 (fully automated).
    install: bool,
    /// What the conversion is doing now (shown by the spinner).
    stage: Arc<Mutex<String>>,
    /// A track file for the Track tab to open (after "Look at it").
    open_request: Option<PathBuf>,
}

impl Default for AcView {
    fn default() -> Self {
        Self {
            tracks_dir: None,
            tracks: Vec::new(),
            filter: String::new(),
            picked: None,
            info: None,
            layout: 0,
            loading: None,
            loaded: None,
            error: None,
            three_d: false,
            cam: Camera::default(),
            opts: ViewOptions { scenery: true, colour_surfaces: false },
            show_ai: true,
            show_markers: true,
            show_textures: true,
            aspect: 1.4,
            fit_pending: false,
            scene: Arc::new(Mutex::new(Scene::default())),
            open_on_start: None,
            nr_name: String::new(),
            making: None,
            made: None,
            make_error: None,
            show_nr: true,
            fewer: false,
            scenery: true,
            scenery_tex: 512,
            install: false,
            stage: Arc::new(Mutex::new(String::new())),
            open_request: None,
        }
    }
}

impl AcView {
    /// Open an AC track folder straight away (command line `--ac <folder> [--layout L] [--3d]`).
    pub fn open_on_start(&mut self, folder: PathBuf, layout: String, three_d: bool) {
        self.open_on_start = Some((folder, layout));
        self.three_d = three_d;
    }

    fn set_tracks_dir(&mut self, dir: PathBuf, settings: &mut Settings) {
        self.tracks = ac::list_tracks(&dir);
        settings.ac_tracks_dir = dir.clone();
        let _ = settings.save();
        self.tracks_dir = Some(dir);
        self.picked = None;
        self.info = None;
    }

    fn pick_track(&mut self, k: usize) {
        self.picked = Some(k);
        self.layout = 0;
        self.error = None;
        self.info = match ac::info(&self.tracks[k].1) {
            Ok(i) => Some(i),
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                None
            }
        };
    }

    fn open(&mut self, ctx: &egui::Context) {
        let (Some(k), Some(info)) = (self.picked, &self.info) else { return };
        let dir = self.tracks[k].1.clone();
        let Some(layout) = info.layouts.get(self.layout).cloned() else { return };
        let title = if info.layouts.len() > 1 { format!("{} — {}", info.title, layout.title) } else { info.title.clone() };
        let opts = self.opts;
        // a name for the NR2003 track: folder (+ layout), letters/digits/_ only
        let raw = if layout.name.is_empty() { format!("{}_ac", info.name) } else { format!("{}_ac", layout.name) };
        self.nr_name = raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c.to_ascii_lowercase() } else { '_' }).collect();
        self.made = None;
        self.make_error = None;
        let (tx, rx) = mpsc::channel();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let r = (|| -> anyhow::Result<Loaded> {
                let tmp = std::env::temp_dir().join(format!("ttk_ac_{}.acscene", std::process::id()));
                let summary = ac::export_scene(&dir, &layout.name, &tmp, TEX_SIDE);
                let scene = summary.and_then(|_| AcScene::read(&tmp, TEX_SIDE));
                let _ = std::fs::remove_file(&tmp);
                let scene = scene?;
                let mesh = ac::build_mesh(&scene, opts);
                let tex = scene.textures.iter().filter(|t| t.image.is_some()).count();
                let summary = format!(
                    "{} pieces of model, {:.1} million triangles, {tex} of {} textures",
                    scene.meshes.len(),
                    scene.triangles() as f64 / 1e6,
                    scene.textures.len()
                );
                Ok(Loaded { title, scene, mesh, summary, dir, layout: layout.name.clone() })
            })();
            let _ = tx.send(r.map_err(|e| format!("{e:#}")));
            ctx2.request_repaint();
        });
        self.loading = Some(rx);
        self.error = None;
    }

    /// "Look at it": a converted track for the Track tab to open.
    pub fn take_open_request(&mut self) -> Option<PathBuf> {
        self.open_request.take()
    }

    fn make_track(&mut self, ctx: &egui::Context, settings: &Settings) {
        let Some(l) = &self.loaded else { return };
        let name = self.nr_name.trim().to_string();
        if let Err(e) = ttk::steps::validate_track_name(&name) {
            self.make_error = Some(format!("{e:#}"));
            return;
        }
        let (dir, layout) = (l.dir.clone(), l.layout.clone());
        let template_dir = ac_template_dir(settings);
        let out = settings.workspace_dir.join(&name);
        let options = if self.fewer { acconv::Options::fewer_pieces() } else { acconv::Options::default() };
        let scenery = self.scenery;
        let sopt = acscenery::Options { tex_side: self.scenery_tex, ..Default::default() };
        let sandbox = settings.sandbox_dir.clone();
        let install = self.install;
        let all_settings = settings.clone();
        let stage = self.stage.clone();
        *stage.lock().unwrap() = "Reading the AC track…".into();
        let (tx, rx) = mpsc::channel();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let say = |m: &str| {
                *stage.lock().unwrap() = m.to_string();
                ctx2.request_repaint();
            };
            let tex_dir = std::env::temp_dir().join(format!("ttk_actex_{}", std::process::id()));
            let r = (|| -> anyhow::Result<Made> {
                let tpl_file = ttk::track::graph::track_file_in(&template_dir)
                    .ok_or_else(|| anyhow::anyhow!("the template track ({}) has no .ptf or .dat", template_dir.display()))?;
                let (bytes, _) = ttk::track::graph::load_ptf_bytes(&tpl_file)?;
                let template = ttk::track::graph::parse_ptf(&bytes)?;
                let tmp = std::env::temp_dir().join(format!("ttk_acconv_{}.acscene", std::process::id()));
                let _ = std::fs::remove_dir_all(&tex_dir);
                if scenery {
                    say("Reading the AC track and its textures…");
                    ac::export_scene_textures(&dir, &layout, &tmp, 1024, &tex_dir)?;
                } else {
                    ac::export_scene(&dir, &layout, &tmp, 0)?;
                }
                let scene = AcScene::read(&tmp, 0);
                let _ = std::fs::remove_file(&tmp);
                let scene = scene?;
                say("Fitting the track…");
                let mut c = acconv::convert(&scene, &template, &options)?;
                let t = ttk::track::model::Track::from_graph(&c.graph)?;
                let scenery_made = if scenery {
                    say("Sorting AC's scenery into objects…");
                    let plan = acscenery::plan(&scene, &t, c.report.offset, c.report.height_base, &sopt);
                    let stats = plan.stats.clone();
                    match acscenery::build(&plan, &tex_dir, &out, &sandbox, &sopt, &|m| say(m)) {
                        Ok(built) => {
                            let objects = acscenery::place(&mut c.graph, &built)?;
                            Some(SceneryMade { stats, objects, mips: built.mips, problems: built.problems })
                        }
                        Err(e) => Some(SceneryMade { stats, objects: 0, mips: 0, problems: vec![format!("{e:#}")] }),
                    }
                } else {
                    None
                };
                say("Adding flaggers, pit stalls and racing-line files…");
                let support = acsupport::finish(&mut c.graph, &scene, c.report.offset, &template, &template_dir, &out, &name)
                    .map_err(|e| format!("{e:#}"));
                say("Saving…");
                let written = acconv::write_track(&c, &out, &name, &template_dir)?;
                let installed = install.then(|| {
                    say("Building and installing into NR2003…");
                    let warnings = Arc::new(Mutex::new(Vec::new()));
                    let (w2, stage2, ctx3) = (warnings.clone(), stage.clone(), ctx2.clone());
                    let log = ttk::logger::Logger::new(move |level, msg| match level {
                        ttk::logger::Level::Warn => w2.lock().unwrap().push(msg),
                        ttk::logger::Level::Step => {
                            *stage2.lock().unwrap() = format!("Building and installing: {}", msg.trim_matches(|c| c == '=' || c == ' '));
                            ctx3.request_repaint();
                        }
                        _ => {}
                    });
                    let r = ttk::steps::build_and_install(&all_settings, &name, &log);
                    let warnings = warnings.lock().unwrap().clone();
                    match r {
                        Ok(()) => Installed {
                            ok: true,
                            message: format!("Installed into {}", all_settings.game_tracks().join(&name).display()),
                            warnings,
                        },
                        Err(e) => Installed { ok: false, message: format!("{e:#}"), warnings },
                    }
                });
                let off = c.report.offset;
                let mut line = Vec::new();
                for s in &t.segments {
                    let n = ((s.length / 4.0).ceil() as usize).max(1);
                    for k in 0..n {
                        let p = s.point(k as f64 / n as f64, 0.0);
                        line.push([(p[0] + off[0]) as f32, (p[1] + off[1]) as f32, 0.0]);
                    }
                }
                Ok(Made { report: c.report, written, scenery: scenery_made, support, installed, line })
            })();
            let _ = std::fs::remove_dir_all(&tex_dir);
            let _ = tx.send(r.map_err(|e| format!("{e:#}")));
            ctx2.request_repaint();
        });
        self.making = Some(rx);
        self.make_error = None;
        self.made = None;
    }

    fn poll_make(&mut self) {
        let Some(rx) = &self.making else { return };
        match rx.try_recv() {
            Ok(Ok(mut m)) => {
                // put the NR2003 centerline at the AC road's height for the 3D view
                if let Some(l) = &self.loaded {
                    let ai = &l.scene.ai;
                    for p in &mut m.line {
                        let near = ai.iter().min_by(|a, b| {
                            let da = (a.pos[0] - p[0]).powi(2) + (a.pos[1] - p[1]).powi(2);
                            let db = (b.pos[0] - p[0]).powi(2) + (b.pos[1] - p[1]).powi(2);
                            da.total_cmp(&db)
                        });
                        p[2] = near.map_or(0.0, |a| a.pos[2]);
                    }
                }
                self.made = Some(m);
                self.making = None;
            }
            Ok(Err(e)) => {
                self.make_error = Some(e);
                self.making = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.make_error = Some("making the track stopped unexpectedly".into());
                self.making = None;
            }
        }
    }

    /// The "Make an NR2003 track" section.
    fn make_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, settings: &mut Settings) {
        ui.separator();
        ui.label(RichText::new("Make an NR2003 track").strong().size(15.0));
        ui.label("Fits NR2003 straights and curves to this track (its shape, width, hills and banking) and saves it in your Sandbox workspace.");
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut self.nr_name).desired_width(200.0));
        });
        // the template: an installed track with a road course's surfaces (watkins_glen by default)
        let tdir = ac_template_dir(settings);
        ui.horizontal(|ui| {
            ui.label("Ground textures from").on_hover_text(
                "An installed NR2003 track. Its asphalt, grass, sand, kerb and wall textures and its track settings are copied. \
                 A road course with all of those (watkins_glen) works best.",
            );
            let current = tdir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            egui::ComboBox::from_id_salt("ac_template").selected_text(&current).width(180.0).height(400.0).show_ui(ui, |ui| {
                for name in installed_tracks(settings) {
                    if ui.selectable_label(name.eq_ignore_ascii_case(&current), &name).clicked() {
                        settings.ac_template = name;
                        let _ = settings.save();
                    }
                }
            });
        });
        let dest = settings.workspace_dir.join(self.nr_name.trim()).join(format!("{}.ptf", self.nr_name.trim()));
        ui.label(RichText::new(format!("Saves to {}", dest.display())).weak());
        if dest.is_file() {
            ui.label(RichText::new("A track with this name is already there. It's backed up before it's replaced.").color(Color32::from_rgb(240, 190, 80)));
        }
        ui.checkbox(&mut self.fewer, "Fewer pieces").on_hover_text(
            "Follows AC a little less closely (within 0.6 m instead of 0.3 m) for far fewer pieces. Try it if a big track has more pieces than NR2003 likes.",
        );
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.scenery, "Bring AC's scenery").on_hover_text(
                "Buildings, stands, trees, fences and the land beyond the road become NR2003 objects, with AC's textures. \
                 Needs Papyrus's make3do and makemip (the Sandbox folder in Settings).",
            );
            ui.add_enabled_ui(self.scenery, |ui| {
                ui.label("textures up to");
                egui::ComboBox::from_id_salt("ac_scenery_tex").selected_text(format!("{} px", self.scenery_tex)).width(80.0).show_ui(ui, |ui| {
                    for side in [256u32, 512, 1024] {
                        ui.selectable_value(&mut self.scenery_tex, side, format!("{side} px"));
                    }
                })
                .response
                .on_hover_text("Bigger is sharper but uses more of the graphics card's memory. 512 suits most tracks.");
            });
        });
        ui.checkbox(&mut self.install, "Then build and install it into NR2003").on_hover_text(
            "Fully automated: after making the track, runs every Build track step (textures, models, pack the .dat) \
             and installs it into NR2003's tracks folder, with track.ini, cameras and .lp files from your template track \
             when it has none. Nothing there is overwritten without a backup. It won't install while NR2003 is running.",
        );
        let go = egui::Button::new(RichText::new(if self.install { "Make, build and install" } else { "Make NR2003 track" }).size(15.0).strong());
        if ui.add_enabled(self.making.is_none() && !self.nr_name.trim().is_empty(), go).clicked() {
            self.make_track(ctx, settings);
        }
        if self.making.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(self.stage.lock().unwrap().clone());
            });
        }
        if let Some(e) = &self.make_error {
            ui.colored_label(Color32::from_rgb(230, 90, 80), e);
        }
        let Some(m) = &self.made else { return };
        let r = &m.report;
        let good = r.crashes.is_empty();
        ui.colored_label(
            if good { Color32::from_rgb(120, 200, 120) } else { Color32::from_rgb(235, 95, 85) },
            if good { "Done. The track passes the checker." } else { "Made, but the checker found problems that will stop it loading:" },
        );
        for c in r.crashes.iter().take(5) {
            ui.colored_label(Color32::from_rgb(235, 95, 85), c);
        }
        egui::Grid::new("ac_made").num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
            ui.label("Pieces");
            ui.label(format!("{} ({} straight, {} curved)", r.pieces, r.straights, r.curves));
            ui.end_row();
            ui.label("Follows AC");
            ui.label(format!("within {:.2} m ({:.2} m on average)", r.worst_m, r.mean_m));
            ui.end_row();
            ui.label("Road height");
            ui.label(format!("within {:.2} m ({:.0}% measured on AC's road)", r.worst_height_m, r.heights_found * 100.0));
            ui.end_row();
            ui.label("Lap");
            ui.label(format!("{:.0} m", r.length_nr));
            ui.end_row();
            ui.label("Ground");
            ui.label(format!(
                "from AC on {} of {} pieces ({})",
                r.ac_ground_pieces,
                r.pieces,
                r.strips_by_surface
                    .iter()
                    .map(|(s, n)| format!("{} {n}", ttk::track::model::surface_name(*s).to_ascii_lowercase()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            ui.end_row();
            ui.label("Walls");
            ui.label(format!("{} where AC has walls, {} at the edge of the ground", r.ac_walls, r.safety_walls));
            ui.end_row();
            ui.label("For racing");
            match &m.support {
                Ok(s) => ui.label(format!(
                    "{} pit stalls and {} grid spots from AC's markers{}, {} flaggers, track.ini and {} racing-line files for the {:.0} m lap",
                    s.pits_from_ac,
                    s.grid_from_ac,
                    if s.moved_onto_ground > 0 { format!(" ({} moved onto the ground)", s.moved_onto_ground) } else { String::new() },
                    s.flaggers.len(),
                    s.lp_files.len(),
                    s.lap_m
                ))
                .on_hover_text(
                    "NR2003 needs these to load the track. The racing lines follow the middle of the road for now, \
                     so AI cars will be slow; real racing lines from AC's AI line come in phase 5.",
                ),
                Err(e) => ui.colored_label(Color32::from_rgb(235, 95, 85), format!("Couldn't make them: {e}")),
            };
            ui.end_row();
            if let Some(sc) = &m.scenery {
                ui.label("Scenery");
                ui.label(format!(
                    "{} objects from {} AC models ({:.2} million triangles), {} textures",
                    sc.objects,
                    sc.stats.meshes,
                    sc.stats.triangles as f64 / 1e6,
                    sc.mips
                ))
                .on_hover_text(format!(
                    "Left out: {} AC models that are the track itself (NR2003 makes its own road and walls), \
                     {} of AC's 3D grass, {} hidden ones, and {} triangles lying on NR2003's own ground.",
                    sc.stats.meshes_track, sc.stats.meshes_grass, sc.stats.meshes_hidden, sc.stats.on_ground
                ));
                ui.end_row();
            }
            if r.warnings > 0 {
                ui.label("Warnings");
                ui.label(format!("{} (see Check in Look at a track)", r.warnings));
                ui.end_row();
            }
        });
        if r.pieces > 660 {
            ui.colored_label(
                Color32::from_rgb(240, 190, 80),
                format!(
                    "This track has {} pieces. The most in any track you have installed is 660 (Circuit Of The Americas), so NR2003 hasn't been seen handling this many. If it won't load, make it again with Fewer pieces.",
                    r.pieces
                ),
            );
        }
        if r.edges_measured > 0 {
            ui.label(RichText::new(format!(
                "AC's AI file had no usable track edges at {} points; they were measured on AC's road instead.",
                r.edges_measured
            )).weak());
        }
        ui.label(RichText::new(format!("Saved {}", m.written.ptf.display())).weak());
        if let Some(i) = &m.installed {
            if i.ok {
                ui.colored_label(Color32::from_rgb(120, 200, 120), format!("{}. Start NR2003 and pick it from the track list.", i.message));
            } else {
                ui.colored_label(Color32::from_rgb(235, 95, 85), format!("Building and installing stopped: {}", i.message));
            }
            for w in i.warnings.iter().take(4) {
                ui.colored_label(Color32::from_rgb(240, 190, 80), w);
            }
        }
        if let Some(sc) = &m.scenery {
            for p in sc.problems.iter().take(4) {
                ui.colored_label(Color32::from_rgb(240, 190, 80), format!("Scenery: {p}"));
            }
            if sc.problems.len() > 4 {
                ui.colored_label(Color32::from_rgb(240, 190, 80), format!("…and {} more scenery problems", sc.problems.len() - 4));
            }
        }
        if !m.written.missing.is_empty() {
            ui.colored_label(Color32::from_rgb(240, 190, 80), format!("Textures not found in the template: {}", m.written.missing.join(", ")));
        }
        ui.horizontal(|ui| {
            if ui.button(RichText::new("Look at it").strong()).clicked() {
                self.open_request = Some(m.written.ptf.clone());
            }
            ui.checkbox(&mut self.show_nr, "Show it over the AC track (orange)");
        });
        ui.label(
            RichText::new(
                "The ground (road, kerbs, grass, sand) and walls follow AC's; their textures come from the track above. \
                 With Bring AC's scenery, AC's buildings, trees and land come along with their own textures. \
                 The AI lines and pit stalls come in the next phase. To try it in NR2003, use Build track with this name.",
            )
            .weak(),
        );
    }

    fn poll(&mut self) {
        let Some(rx) = &self.loading else { return };
        match rx.try_recv() {
            Ok(Ok(l)) => {
                {
                    let mut sc = self.scene.lock().unwrap();
                    sc.clear_textures = true;
                    sc.pending_tex = l
                        .scene
                        .textures
                        .iter()
                        .filter_map(|t| t.image.clone().map(|i| (t.name.to_ascii_lowercase(), i)))
                        .collect();
                    sc.pending_objects = Some(Vec::new());
                }
                self.loaded = Some(l);
                self.upload();
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

    fn upload(&mut self) {
        if let Some(l) = &mut self.loaded {
            self.scene.lock().unwrap().pending =
                Some((std::mem::take(&mut l.mesh.verts), std::mem::take(&mut l.mesh.indices), l.mesh.batches.clone()));
        }
    }

    fn rebuild(&mut self) {
        if let Some(l) = &mut self.loaded {
            l.mesh = ac::build_mesh(&l.scene, self.opts);
        }
        self.upload();
    }

    /// Frame the track itself: the AI line plus a margin (AC tracks have far-off scenery
    /// backdrops that would make the track tiny), else everything.
    fn fit(&mut self) {
        if let Some(l) = &self.loaded {
            let o = l.mesh.origin;
            let (cx, cy, w, h, z) = if l.scene.ai.len() > 1 {
                let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
                for p in &l.scene.ai {
                    for k in 0..3 {
                        lo[k] = lo[k].min(p.pos[k] as f64);
                        hi[k] = hi[k].max(p.pos[k] as f64);
                    }
                }
                let margin = 60.0;
                (0.5 * (lo[0] + hi[0]) - o[0], 0.5 * (lo[1] + hi[1]) - o[1], hi[0] - lo[0] + 2.0 * margin, hi[1] - lo[1] + 2.0 * margin, 0.5 * (lo[2] + hi[2]) - o[2])
            } else {
                let [w, h, z0, z1] = l.mesh.extent;
                (0.0, 0.0, w, h, 0.5 * (z1 - z0))
            };
            self.cam.cx = cx;
            self.cam.cy = cy;
            self.cam.half_h = 0.5 * h.max(w / self.aspect.max(0.1)).max(20.0);
            self.cam.target = [cx, cy, z];
            self.cam.dist = 1.0 * w.max(h).max(20.0);
            self.cam.yaw = -1.9;
            self.cam.pitch = 0.6;
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, settings: &mut Settings) {
        if self.tracks_dir.is_none() {
            let dir = Some(settings.ac_tracks_dir.clone()).filter(|d| d.is_dir()).or_else(ac::find_tracks_dir);
            if let Some(d) = dir {
                self.tracks = ac::list_tracks(&d);
                self.tracks_dir = Some(d);
            }
        }
        if let Some((folder, layout)) = self.open_on_start.take() {
            if let Some(parent) = folder.parent() {
                self.tracks = ac::list_tracks(parent);
                self.tracks_dir = Some(parent.to_path_buf());
            }
            if let Some(k) = self.tracks.iter().position(|t| t.1 == folder) {
                self.pick_track(k);
                if let Some(i) = self.info.as_ref().and_then(|i| i.layouts.iter().position(|l| l.name == layout)) {
                    self.layout = i;
                }
                self.open(ctx);
            }
        }
        self.poll();
        self.poll_make();
        egui::SidePanel::left("ac_side").resizable(true).default_width(370.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.side_panel(ui, ctx, settings));
        });
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| self.viewport(ui));
    }

    fn side_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, settings: &mut Settings) {
        ui.add_space(6.0);
        ui.label(RichText::new("Assetto Corsa track").size(18.0).strong());
        ui.label("Look at an Assetto Corsa track before turning it into an NR2003 track.");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("AC tracks folder");
            if ui.button("Choose…").clicked() {
                let mut dlg = rfd::FileDialog::new().set_title("Pick Assetto Corsa's content\\tracks folder");
                if let Some(d) = &self.tracks_dir {
                    dlg = dlg.set_directory(d);
                }
                if let Some(d) = dlg.pick_folder() {
                    self.set_tracks_dir(d, settings);
                }
            }
        });
        match &self.tracks_dir {
            Some(d) => ui.label(RichText::new(d.display().to_string()).weak()),
            None => ui.colored_label(Color32::from_rgb(240, 190, 80), "Assetto Corsa wasn't found. Use Choose… to pick its content\\tracks folder."),
        };

        if !self.tracks.is_empty() {
            ui.add_space(4.0);
            let mut pick = None;
            ui.horizontal(|ui| {
                let text = self.picked.map(|k| self.tracks[k].0.clone()).unwrap_or_else(|| format!("Choose a track ({})…", self.tracks.len()));
                egui::ComboBox::from_id_salt("ac_track").selected_text(text).width(230.0).height(460.0).show_ui(ui, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Find").desired_width(200.0));
                    let f = self.filter.to_ascii_lowercase();
                    for (k, (name, _)) in self.tracks.iter().enumerate() {
                        if !f.is_empty() && !name.to_ascii_lowercase().contains(&f) {
                            continue;
                        }
                        if ui.selectable_label(self.picked == Some(k), name).clicked() {
                            pick = Some(k);
                        }
                    }
                });
            });
            if let Some(k) = pick {
                self.pick_track(k);
            }
        }

        if let Some(info) = &self.info {
            if info.layouts.len() > 1 {
                ui.horizontal(|ui| {
                    ui.label("Layout");
                    let cur = info.layouts.get(self.layout).map(|l| l.title.clone()).unwrap_or_default();
                    egui::ComboBox::from_id_salt("ac_layout").selected_text(cur).width(200.0).show_ui(ui, |ui| {
                        for (k, l) in info.layouts.iter().enumerate() {
                            ui.selectable_value(&mut self.layout, k, &l.title);
                        }
                    });
                });
            }
            if let Some(l) = info.layouts.get(self.layout) {
                let missing: Vec<&str> = l.models.iter().filter(|m| !m.exists).map(|m| m.file.as_str()).collect();
                if !missing.is_empty() {
                    ui.colored_label(Color32::from_rgb(240, 190, 80), format!("Missing model files: {}", missing.join(", ")));
                }
                if l.ai_points == 0 {
                    ui.colored_label(Color32::from_rgb(240, 190, 80), "This layout has no AI line (ai\\fast_lane.ai); converting needs one.");
                }
            }
            let open = egui::Button::new(RichText::new("Open").size(15.0).strong());
            if ui.add_enabled(self.loading.is_none(), open).clicked() {
                self.open(ctx);
            }
        }
        if self.loading.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the track (big tracks take a few seconds)…");
            });
        }
        if let Some(e) = &self.error {
            ui.colored_label(Color32::from_rgb(230, 90, 80), e);
        }
        let Some(l) = &self.loaded else { return };

        ui.separator();
        let s = &l.scene;
        let lap = s.ai.last().map_or(0.0, |p| p.distance as f64);
        let widths: Vec<f32> = s.ai.iter().map(|p| p.left + p.right).filter(|w| *w > 0.0).collect();
        let avg_w = if widths.is_empty() { 0.0 } else { widths.iter().sum::<f32>() / widths.len() as f32 };
        egui::Grid::new("ac_facts").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            ui.label("Track");
            ui.label(RichText::new(&l.title).strong());
            ui.end_row();
            ui.label("Lap");
            if lap > 0.0 {
                ui.label(format!("{:.3} miles  ({:.0} m, from the AI line)", lap / ttk::track::model::METRES_PER_MILE, lap));
            } else {
                ui.label("no AI line");
            }
            ui.end_row();
            if avg_w > 0.0 {
                ui.label("Width");
                ui.label(format!("{avg_w:.1} m on average"));
                ui.end_row();
            }
            ui.label("Pit boxes");
            ui.label(format!("{}", s.markers_named("AC_PIT_").count()));
            ui.end_row();
            ui.label("Grid spots");
            ui.label(format!("{}", s.markers_named("AC_START_").count()));
            ui.end_row();
            ui.label("Model");
            ui.label(&l.summary);
            ui.end_row();
        });
        let missing_tex = s.textures.iter().filter(|t| t.image.is_none()).count();
        if missing_tex > 0 {
            ui.label(
                RichText::new(format!(
                    "{missing_tex} textures use a newer compression (BC7) that isn't read yet; they show in plain colour."
                ))
                .weak(),
            );
        }

        ui.add_space(6.0);
        let mut changed = false;
        ui.horizontal(|ui| {
            for (v, label) in [(false, "Map (from above)"), (true, "3D")] {
                if ui.add(egui::Button::new(RichText::new(label).size(15.0).strong()).selected(self.three_d == v)).clicked() {
                    self.three_d = v;
                }
            }
            if ui.button("Fit to screen").clicked() {
                self.fit();
            }
        });
        ui.horizontal_wrapped(|ui| {
            changed |= ui.checkbox(&mut self.opts.scenery, "Scenery").on_hover_text("Buildings, stands, trees and everything else cars don't drive on.").changed();
            changed |= ui
                .checkbox(&mut self.opts.colour_surfaces, "Colour what cars drive on")
                .on_hover_text("Road, kerbs, grass, sand, walls and pit lane in plain colours, including the hidden ones AC uses only for driving.")
                .changed();
            ui.checkbox(&mut self.show_ai, "AI line and edges");
            ui.checkbox(&mut self.show_markers, "Pits and grid");
            ui.checkbox(&mut self.show_textures, "Textures");
        });
        if changed {
            self.rebuild();
        }
        self.make_panel(ui, ctx, settings);

        // Driving surfaces, in plain words
        let Some(l) = &self.loaded else { return };
        ui.separator();
        ui.label(RichText::new("What cars drive on").strong());
        let s = &l.scene;
        for (k, sf) in s.surfaces.iter().enumerate() {
            let n = s.meshes.iter().filter(|m| m.surface == Some(k)).count();
            if n == 0 {
                continue;
            }
            let c = ac::surface_colour(&sf.key, sf.valid);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::hover());
                ui.painter().rect_filled(r, 2.0, Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8));
                let what = if sf.pitlane {
                    "pit lane"
                } else if sf.key == "WALL" {
                    "wall"
                } else if sf.valid {
                    "counts as track"
                } else {
                    "off the track"
                };
                ui.label(format!("{}  ({what}, {n} pieces)", sf.key));
            });
        }
    }

    fn viewport(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let sky = if self.three_d { Color32::from_rgb(120, 160, 205) } else { Color32::from_rgb(28, 32, 36) };
        painter.rect_filled(rect, 0.0, sky);
        self.aspect = (rect.width() / rect.height().max(1.0)) as f64;
        if self.fit_pending && self.loaded.is_some() {
            self.fit_pending = false;
            self.fit();
        }
        let Some(l) = &self.loaded else {
            let msg = if self.loading.is_some() { "Reading the track…" } else { "Pick an Assetto Corsa track on the left and press Open" };
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(20.0), Color32::from_gray(200));
            return;
        };
        camera_input(&mut self.cam, self.three_d, ui, &resp, rect, true);
        let mvp = if self.three_d { orbit_matrix(&self.cam, self.aspect, &l.mesh) } else { map_matrix(&self.cam, self.aspect, &l.mesh) };
        let scene = self.scene.clone();
        let textured = self.show_textures;
        let cb = egui_glow::CallbackFn::new(move |_info, p| {
            scene.lock().unwrap().paint(p.gl(), &mvp, -10.0, textured);
        });
        painter.add(egui::PaintCallback { rect, callback: Arc::new(cb) });

        let o = l.mesh.origin;
        let to_screen = |p: [f32; 3]| -> Option<egui::Pos2> {
            let v = [(p[0] as f64 - o[0]) as f32, (p[1] as f64 - o[1]) as f32, (p[2] as f64 - o[2]) as f32 + 0.3, 1.0];
            let c = mul_vec(&mvp, v);
            if c[3] <= 0.0 {
                return None;
            }
            Some(egui::pos2(rect.left() + (c[0] / c[3] + 1.0) * 0.5 * rect.width(), rect.top() + (1.0 - c[1] / c[3]) * 0.5 * rect.height()))
        };
        let s = &l.scene;
        if self.show_ai && s.ai.len() > 1 {
            // the AI line, and the track edges beside it (left = 90° anticlockwise of travel)
            let n = s.ai.len();
            let mut line = Vec::with_capacity(n + 1);
            let mut left = Vec::with_capacity(n + 1);
            let mut right = Vec::with_capacity(n + 1);
            for k in 0..=n {
                let p = s.ai[k % n];
                let q = s.ai[(k + 1) % n];
                let (dx, dy) = (q.pos[0] - p.pos[0], q.pos[1] - p.pos[1]);
                let len = (dx * dx + dy * dy).sqrt().max(1e-6);
                let (lx, ly) = (-dy / len, dx / len);
                line.push(to_screen(p.pos));
                left.push(to_screen([p.pos[0] + lx * p.left, p.pos[1] + ly * p.left, p.pos[2]]));
                right.push(to_screen([p.pos[0] - lx * p.right, p.pos[1] - ly * p.right, p.pos[2]]));
            }
            let draw = |pts: &[Option<egui::Pos2>], stroke: egui::Stroke| {
                for w in pts.windows(2) {
                    if let (Some(a), Some(b)) = (w[0], w[1]) {
                        if rect.expand(200.0).contains(a) || rect.expand(200.0).contains(b) {
                            painter.line_segment([a, b], stroke);
                        }
                    }
                }
            };
            draw(&left, egui::Stroke::new(1.5f32, Color32::from_rgb(255, 255, 255)));
            draw(&right, egui::Stroke::new(1.5f32, Color32::from_rgb(255, 255, 255)));
            draw(&line, egui::Stroke::new(2.0f32, Color32::from_rgb(80, 200, 255)));
            if let Some(q) = to_screen(s.ai[0].pos) {
                painter.circle_filled(q, 5.0, Color32::WHITE);
                painter.text(q - egui::vec2(0.0, 8.0), egui::Align2::CENTER_BOTTOM, "Start / finish", egui::FontId::proportional(13.0), Color32::WHITE);
            }
        }
        if let Some(m) = self.made.as_ref().filter(|_| self.show_nr) {
            let pts: Vec<Option<egui::Pos2>> = m.line.iter().map(|p| to_screen(*p)).collect();
            for k in 0..pts.len() {
                if let (Some(a), Some(b)) = (pts[k], pts[(k + 1) % pts.len()]) {
                    painter.line_segment([a, b], egui::Stroke::new(2.0f32, Color32::from_rgb(255, 150, 40)));
                }
            }
        }
        if self.show_markers {
            for m in &s.markers {
                let n = m.name.to_ascii_uppercase();
                let col = if n.starts_with("AC_PIT_") {
                    Color32::from_rgb(90, 140, 255)
                } else if n.starts_with("AC_START_") {
                    Color32::from_rgb(90, 220, 110)
                } else {
                    continue;
                };
                if let Some(q) = to_screen(m.pos) {
                    painter.circle_filled(q, 3.5, col);
                }
            }
        }
        let hint = if self.three_d {
            "Drag to turn  ·  Right-drag to move  ·  Scroll to zoom  ·  Blue: AI line  ·  White: track edges  ·  Blue dots: pit boxes  ·  Green dots: grid"
        } else {
            "Drag to move  ·  Scroll to zoom  ·  Blue: AI line  ·  White: track edges  ·  Blue dots: pit boxes  ·  Green dots: grid"
        };
        painter.text(rect.left_top() + egui::vec2(10.0, 8.0), egui::Align2::LEFT_TOP, hint, egui::FontId::proportional(13.0), Color32::from_gray(235));
    }
}

/// The installed track AC conversions copy textures and settings from: the chosen one
/// (watkins_glen by default) if it's installed, else the Build tab's template.
fn ac_template_dir(settings: &Settings) -> PathBuf {
    let chosen = settings.game_tracks().join(settings.ac_template.trim());
    if !settings.ac_template.trim().is_empty() && ttk::track::graph::track_file_in(&chosen).is_some() {
        chosen
    } else {
        settings.game_tracks().join(settings.template_track.trim())
    }
}

/// Installed NR2003 tracks (folders with a .ptf or .dat), sorted.
fn installed_tracks(settings: &Settings) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(settings.game_tracks())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && ttk::track::graph::track_file_in(p).is_some())
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .filter(|n| !n.eq_ignore_ascii_case("shared"))
        .collect();
    v.sort_by_key(|n| n.to_ascii_lowercase());
    v
}

#[cfg(test)]
#[path = "ac_ui_tests.rs"]
mod ui_tests;
