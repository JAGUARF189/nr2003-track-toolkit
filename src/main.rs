#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Color32, RichText};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use ttk::config::{Resize, Settings, TexDest, TexJob, TrackOpts};
use ttk::logger::{Level, Logger};
use ttk::steps::{run_all_steps, run_steps, validate_track_name, Ctx, Step};

mod textures_ui;
mod track_ui;

const MAX_LOG_LINES: usize = 10_000;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Build,
    Textures,
    Track,
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("NR2003 Track Toolkit")
            .with_inner_size([1060.0, 780.0])
            .with_min_inner_size([720.0, 480.0])
            .with_drag_and_drop(true),
        depth_buffer: 24, // the Track tab's 3D view
        ..Default::default()
    };
    eframe::run_native(
        "NR2003 Track Toolkit",
        options,
        Box::new(|cc| {
            let mut app = App::new();
            // `track-toolkit.exe <track.ptf|.dat> [--3d] [--piece N] [--wall N | --strip N] [--objects]` opens straight into the Track tab.
            let args: Vec<String> = std::env::args().skip(1).collect();
            if let Some(file) = args.iter().find(|a| {
                let l = a.to_ascii_lowercase();
                l.ends_with(".ptf") || l.ends_with(".dat")
            }) {
                app.tab = Tab::Track;
                let piece = args
                    .iter()
                    .position(|a| a == "--piece")
                    .and_then(|k| args.get(k + 1))
                    .and_then(|n| n.parse::<usize>().ok())
                    .and_then(|n| n.checked_sub(1));
                app.track_view.open_path(&cc.egui_ctx, PathBuf::from(file), args.iter().any(|a| a == "--3d"), piece);
                if args.iter().any(|a| a == "--objects") {
                    app.track_view.objects_mode();
                }
                // --wall N / --strip N: pick the Nth wall or ground strip (right to left) of --piece
                let num = |flag: &str| {
                    args.iter().position(|a| a == flag).and_then(|k| args.get(k + 1)).and_then(|n| n.parse::<usize>().ok())
                };
                // --dist M / --yaw D / --pitch D: 3D camera, for screenshots
                let real = |flag: &str| {
                    args.iter().position(|a| a == flag).and_then(|k| args.get(k + 1)).and_then(|n| n.parse::<f64>().ok())
                };
                app.track_view.camera_on_load(real("--dist"), real("--yaw"), real("--pitch"));
                if let Some(n) = num("--wall") {
                    app.track_view.pick_on_load(true, n);
                } else if let Some(n) = num("--strip") {
                    app.track_view.pick_on_load(false, n);
                }
            }
            Ok(Box::new(app))
        }),
    )
}

enum Msg {
    Log(Level, String),
    Start(Step),
    End(Step, bool),
    Finished,
}

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Idle,
    Running,
    Ok,
    Failed,
}

struct App {
    settings: Settings,
    tracks: Vec<String>,
    game_tracks: Vec<String>,
    track: String,
    new_name: String,
    opts: TrackOpts,
    logs: Vec<(Level, String)>,
    rx: Option<mpsc::Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
    status: [Status; 6],
    show_settings: bool,
    show_tool_output: bool,
    tab: Tab,
    textures: textures_ui::TexturesView,
    track_view: track_ui::TrackView,
}

impl App {
    fn new() -> Self {
        let settings = Settings::load();
        let tracks = settings.list_tracks();
        let track = if tracks.contains(&settings.last_track) {
            settings.last_track.clone()
        } else {
            tracks.first().cloned().unwrap_or_default()
        };
        let opts = if track.is_empty() {
            TrackOpts::default()
        } else {
            TrackOpts::load(&settings.track_dir(&track))
        };
        let show_settings =
            !settings.sandbox_dir.join("mklib.exe").is_file() || !settings.game_tracks().is_dir();
        let game_tracks = settings.list_game_tracks();
        let mut app = Self {
            settings,
            tracks,
            game_tracks,
            track,
            new_name: String::new(),
            opts,
            logs: Vec::new(),
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            status: [Status::Idle; 6],
            show_settings,
            show_tool_output: true,
            tab: Tab::Build,
            textures: textures_ui::TexturesView::default(),
            track_view: track_ui::TrackView::default(),
        };
        app.push(
            Level::Info,
            "Pick or create a track, then run the steps one at a time or all at once.",
        );
        app
    }

    fn busy(&self) -> bool {
        self.rx.is_some()
    }

    fn push(&mut self, l: Level, m: impl Into<String>) {
        self.logs.push((l, m.into()));
        if self.logs.len() > MAX_LOG_LINES {
            let extra = self.logs.len() - MAX_LOG_LINES;
            self.logs.drain(..extra);
        }
    }

    fn save_opts(&mut self) {
        if self.track.is_empty() {
            return;
        }
        let td = self.settings.track_dir(&self.track);
        if td.is_dir() {
            if let Err(e) = self.opts.save(&td) {
                self.push(Level::Warn, format!("Couldn't save track options: {e:#}"));
            }
        }
    }

    fn select_track(&mut self, name: String) {
        if name == self.track {
            return;
        }
        self.save_opts();
        self.opts = TrackOpts::load(&self.settings.track_dir(&name));
        self.track = name;
        self.status = [Status::Idle; 6];
        self.settings.last_track = self.track.clone();
        let _ = self.settings.save();
    }

    fn refresh_lists(&mut self) {
        self.tracks = self.settings.list_tracks();
        self.game_tracks = self.settings.list_game_tracks();
    }

    fn start(&mut self, ctx: &egui::Context, steps: Vec<Step>) {
        if self.busy() || steps.is_empty() {
            return;
        }
        if let Err(e) = validate_track_name(&self.track) {
            self.push(Level::Error, format!("{e:#}"));
            return;
        }
        self.save_opts();
        self.settings.last_track = self.track.clone();
        let _ = self.settings.save();
        for s in &steps {
            self.status[s.index()] = Status::Idle;
        }

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.cancel.store(false, Ordering::Relaxed);
        let settings = self.settings.clone();
        let name = self.track.clone();
        let opts = self.opts.clone();
        let cancel = self.cancel.clone();
        let egui_ctx = ctx.clone();

        std::thread::spawn(move || {
            let tx_log = tx.clone();
            let repaint = egui_ctx.clone();
            let log = Logger::new(move |l, m| {
                let _ = tx_log.send(Msg::Log(l, m));
                repaint.request_repaint();
            });
            let c = Ctx {
                settings: &settings,
                name: &name,
                opts: &opts,
                log: &log,
                cancel: &cancel,
            };
            let res = run_steps(&steps, &c, |s, r| {
                let _ = tx.send(match r {
                    None => Msg::Start(s),
                    Some(r) => Msg::End(s, r.is_ok()),
                });
            });
            if res.is_ok() {
                log.ok("All done.");
            }
            let _ = tx.send(Msg::Finished);
            egui_ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        let msgs: Vec<Msg> = match &self.rx {
            Some(rx) => rx.try_iter().collect(),
            None => return,
        };
        for m in msgs {
            match m {
                Msg::Log(l, s) => self.push(l, s),
                Msg::Start(s) => self.status[s.index()] = Status::Running,
                Msg::End(s, ok) => {
                    self.status[s.index()] = if ok { Status::Ok } else { Status::Failed }
                }
                Msg::Finished => {
                    self.rx = None;
                    self.refresh_lists();
                    // A step may have created toolkit.json; reload nothing, keep UI state.
                }
            }
        }
    }
}

// ---------- small widgets ----------

fn open_in_explorer(p: &Path) {
    let _ = std::process::Command::new("explorer.exe").arg(p).spawn();
}

fn path_edit(ui: &mut egui::Ui, p: &mut PathBuf, pick_dir: bool) -> bool {
    let mut s = p.display().to_string();
    let mut changed = false;
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 70.0).max(120.0);
        if ui
            .add(egui::TextEdit::singleline(&mut s).desired_width(w))
            .changed()
        {
            *p = PathBuf::from(s.trim());
            changed = true;
        }
        if ui.button("Browse…").clicked() {
            let mut d = rfd::FileDialog::new();
            if p.is_dir() {
                d = d.set_directory(&*p);
            }
            let picked = if pick_dir {
                d.pick_folder()
            } else {
                d.pick_file()
            };
            if let Some(x) = picked {
                *p = x;
                changed = true;
            }
        }
    });
    changed
}

/// Edit a path stored as a string relative to `base`.
fn rel_path_edit(ui: &mut egui::Ui, s: &mut String, base: &Path, hint: &str) {
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 130.0).max(120.0);
        ui.add(
            egui::TextEdit::singleline(s)
                .desired_width(w)
                .hint_text(hint),
        );
        if ui.button("Browse…").clicked() {
            let start = ttk::util::resolve(base, s);
            let mut d = rfd::FileDialog::new();
            if start.is_dir() {
                d = d.set_directory(&start);
            } else if base.is_dir() {
                d = d.set_directory(base);
            }
            if let Some(x) = d.pick_folder() {
                *s = match x.strip_prefix(base) {
                    Ok(rel) => rel.display().to_string(),
                    Err(_) => x.display().to_string(),
                };
            }
        }
        if ui.button("Open").clicked() {
            let p = ttk::util::resolve(base, s);
            if p.exists() {
                open_in_explorer(&p);
            }
        }
    });
}

fn status_chip(ui: &mut egui::Ui, st: Status) {
    let (txt, col) = match st {
        Status::Idle => ("  ·  ", Color32::GRAY),
        Status::Running => (" RUN ", Color32::from_rgb(90, 160, 255)),
        Status::Ok => ("  OK ", Color32::from_rgb(80, 190, 100)),
        Status::Failed => (" FAIL", Color32::from_rgb(230, 80, 70)),
    };
    ui.label(RichText::new(txt).monospace().strong().color(col));
}

fn log_color(l: Level, dark: bool) -> Color32 {
    match l {
        Level::Step => {
            if dark {
                Color32::WHITE
            } else {
                Color32::BLACK
            }
        }
        Level::Info => {
            if dark {
                Color32::from_gray(210)
            } else {
                Color32::from_gray(40)
            }
        }
        Level::Ok => Color32::from_rgb(70, 180, 90),
        Level::Warn => Color32::from_rgb(220, 160, 40),
        Level::Error => Color32::from_rgb(230, 80, 70),
        Level::Tool => Color32::from_gray(130),
    }
}

// ---------- UI ----------

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        // Track tab saving/autosave runs on every tab; ask before closing with unsaved changes.
        self.track_view.tick(ctx, &self.settings);
        if ctx.input(|i| i.viewport().close_requested()) && self.track_view.on_close_requested() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        if self.track_view.take_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let busy = self.busy();
        let mut run: Option<Vec<Step>> = None;

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for (t, label) in [(Tab::Build, "Build track"), (Tab::Textures, "Textures"), (Tab::Track, "Look at a track")] {
                    let b = egui::Button::new(RichText::new(label).size(16.0).strong())
                        .selected(self.tab == t);
                    if ui.add(b).clicked() {
                        self.tab = t;
                    }
                }
            });
            ui.separator();
            // The build bar (track, new track, Run all, Settings) isn't used by the Track tab.
            if self.tab != Tab::Track {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Track").strong());
                let mut pick: Option<String> = None;
                ui.add_enabled_ui(!busy, |ui| {
                    egui::ComboBox::from_id_salt("track_pick")
                        .width(200.0)
                        .selected_text(if self.track.is_empty() {
                            "(none)"
                        } else {
                            self.track.as_str()
                        })
                        .show_ui(ui, |ui| {
                            for t in &self.tracks {
                                if ui.selectable_label(*t == self.track, t).clicked() {
                                    pick = Some(t.clone());
                                }
                            }
                        });
                    ui.separator();
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_name)
                            .desired_width(150.0)
                            .hint_text("new track name"),
                    );
                    let valid = validate_track_name(self.new_name.trim()).is_ok();
                    if ui.add_enabled(valid, egui::Button::new("Create")).clicked() {
                        pick = Some(self.new_name.trim().to_string());
                        self.new_name.clear();
                        run = Some(vec![Step::Workspace]);
                    }
                });
                if let Some(p) = pick {
                    self.select_track(p);
                }
                ui.separator();
                let can_run = !busy && !self.track.is_empty();
                if ui
                    .add_enabled(
                        can_run,
                        egui::Button::new(RichText::new("▶ Run all").strong()),
                    )
                    .on_hover_text(
                        "Workspace -> Textures -> Models -> Pack -> Install (+ Stalls if enabled)",
                    )
                    .clicked()
                {
                    run = Some(run_all_steps(&self.opts));
                }
                if busy && ui.button("■ Cancel").clicked() {
                    self.cancel.store(true, Ordering::Relaxed);
                }
                ui.separator();
                if ui
                    .selectable_label(self.show_settings, "⚙ Settings")
                    .clicked()
                {
                    self.show_settings = !self.show_settings;
                }
            });
            }
            ui.add_space(4.0);
        });

        // The log is for build steps; the Textures tab reports in its own panel.
        if self.tab == Tab::Build {
            egui::TopBottomPanel::bottom("log")
                .resizable(true)
                .default_height(260.0)
                .min_height(120.0)
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Log").strong());
                        ui.checkbox(&mut self.show_tool_output, "show tool output");
                        if ui.button("Clear").clicked() {
                            self.logs.clear();
                        }
                        if ui.button("Copy").clicked() {
                            let text: String = self
                                .logs
                                .iter()
                                .map(|(_, s)| s.as_str())
                                .collect::<Vec<_>>()
                                .join("\r\n");
                            ui.ctx().copy_text(text);
                        }
                        if busy {
                            ui.spinner();
                        }
                    });
                    ui.separator();
                    let dark = ui.visuals().dark_mode;
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for (l, s) in &self.logs {
                                if *l == Level::Tool && !self.show_tool_output {
                                    continue;
                                }
                                let mut t = RichText::new(s).monospace().color(log_color(*l, dark));
                                if *l == Level::Step {
                                    t = t.strong();
                                }
                                ui.label(t);
                            }
                        });
                });
        }

        if self.tab == Tab::Textures {
            let td = self.settings.track_dir(&self.track);
            let mut places = Vec::new();
            if !self.track.is_empty() {
                places.push(("This track".to_string(), td.clone()));
                places.push(("Track surfaces".to_string(), td.join("trackmat")));
            }
            places.push(("Shared".to_string(), self.settings.shared_dir()));
            places.push((
                "Game shared".to_string(),
                self.settings.game_tracks().join("shared"),
            ));
            let game_dats: Vec<(String, PathBuf)> = self
                .game_tracks
                .iter()
                .map(|t| {
                    (
                        t.clone(),
                        self.settings.game_tracks().join(t).join(format!("{t}.dat")),
                    )
                })
                .collect();
            let unpack_root = self
                .settings
                .workspace_dir
                .parent()
                .unwrap_or(&self.settings.workspace_dir)
                .join("Unpacked");
            let env = textures_ui::TexturesEnv {
                places: &places,
                game_dats: &game_dats,
                unpack_root: &unpack_root,
                sandbox: &self.settings.sandbox_dir,
            };
            self.textures.show(ctx, &env);
        } else if self.tab == Tab::Track {
            let folders: Vec<(String, PathBuf)> = self
                .game_tracks
                .iter()
                .map(|t| (t.clone(), self.settings.game_tracks().join(t)))
                .collect();
            let workspace = self.settings.workspace_dir.clone();
            self.track_view.show(
                ctx,
                &track_ui::TrackEnv { game_tracks: &folders, workspace: &workspace },
                &mut self.settings,
            );
        } else {
            self.build_tab(ctx, busy, &mut run);
        }

        if self.show_settings {
            self.settings_window(ctx);
        }

        if let Some(steps) = run {
            self.start(ctx, steps);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_opts();
        let _ = self.settings.save();
    }
}

impl App {
    fn build_tab(&mut self, ctx: &egui::Context, busy: bool, run: &mut Option<Vec<Step>>) {
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.track.is_empty() {
                ui.add_space(20.0);
                ui.heading("No track yet");
                ui.label("Type a name at the top and press Create. That builds the Sandbox workspace for it.");
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for step in Step::ALL {
                    self.step_section(ui, step, busy, run);
                    ui.add_space(2.0);
                }
            });
        });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut changed = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .default_width(620.0)
            .show(ctx, |ui| {
                egui::Grid::new("settings_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                    ui.label("Sandbox tools folder");
                    ui.vertical(|ui| {
                        changed |= path_edit(ui, &mut self.settings.sandbox_dir, true);
                        let missing: Vec<&str> = ["mklib.exe", "makemip.exe", "make3do.exe", "mkstalls.exe"]
                            .into_iter()
                            .filter(|t| !self.settings.sandbox_dir.join(t).is_file())
                            .collect();
                        if missing.is_empty() {
                            ui.label(RichText::new("mklib, makemip, make3do, mkstalls found").color(Color32::from_rgb(70, 180, 90)));
                        } else {
                            ui.label(RichText::new(format!("missing: {}", missing.join(", "))).color(Color32::from_rgb(230, 80, 70)));
                        }
                    });
                    ui.end_row();

                    ui.label("NR2003 folder");
                    ui.vertical(|ui| {
                        changed |= path_edit(ui, &mut self.settings.nr2003_dir, true);
                        if !self.settings.game_tracks().is_dir() {
                            ui.label(RichText::new("no tracks\\ folder here").color(Color32::from_rgb(230, 80, 70)));
                        }
                    });
                    ui.end_row();

                    ui.label("Workspace (Level-1 tracks folder)");
                    ui.vertical(|ui| {
                        changed |= path_edit(ui, &mut self.settings.workspace_dir, true);
                        ui.label(
                            RichText::new("Where Sandbox work lives: tracks\\shared and tracks\\<name>\\trackmat. Keep it outside the game folder.")
                                .small()
                                .weak(),
                        );
                    });
                    ui.end_row();

                    ui.label("Template track");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("template_pick")
                            .width(220.0)
                            .selected_text(self.settings.template_track.as_str())
                            .show_ui(ui, |ui| {
                                for t in &self.game_tracks {
                                    if ui.selectable_label(*t == self.settings.template_track, t).clicked() {
                                        self.settings.template_track = t.clone();
                                        changed = true;
                                    }
                                }
                            });
                        ui.label(RichText::new("source of track.ini, .cam, .sim, .lp for new tracks").small().weak());
                    });
                    ui.end_row();
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Saved to {}", Settings::default_path().display())).small().weak());
                });
            });
        if changed {
            let _ = self.settings.save();
            self.refresh_lists();
            if !self.tracks.contains(&self.track) {
                let next = self.tracks.first().cloned().unwrap_or_default();
                self.track = String::new();
                if !next.is_empty() {
                    self.select_track(next);
                }
            }
        }
        self.show_settings = open;
    }

    fn step_section(
        &mut self,
        ui: &mut egui::Ui,
        step: Step,
        busy: bool,
        run: &mut Option<Vec<Step>>,
    ) {
        let td = self.settings.track_dir(&self.track);
        let gd = self.settings.game_tracks().join(&self.track);
        let st = self.status[step.index()];

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                status_chip(ui, st);
                ui.label(RichText::new(step.title()).strong().size(15.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Run")).clicked() {
                        *run = Some(vec![step]);
                    }
                    let folder = match step {
                        Step::Install | Step::Stalls => gd.clone(),
                        _ => td.clone(),
                    };
                    if ui
                        .add_enabled(folder.exists(), egui::Button::new("Open folder"))
                        .clicked()
                    {
                        open_in_explorer(&folder);
                    }
                });
            });

            ui.add_enabled_ui(!busy, |ui| match step {
                Step::Workspace => self.ui_workspace(ui, &td),
                Step::Textures => self.ui_textures(ui, &td),
                Step::Models => self.ui_models(ui, &td),
                Step::Pack => self.ui_pack(ui, &td),
                Step::Install => self.ui_install(ui, &gd),
                Step::Stalls => self.ui_stalls(ui),
            });
        });
    }

    fn ui_workspace(&mut self, ui: &mut egui::Ui, td: &Path) {
        ui.label(
            RichText::new(format!(
                "Creates {}\\trackmat and unpacks shared.dat into {}.",
                td.display(),
                self.settings.shared_dir().display()
            ))
            .weak(),
        );
        ui.checkbox(
            &mut self.opts.copy_flagger_mips,
            "Copy flagger textures (flag_*.mip, series_flagger, stopgo) into the track folder",
        );
        ui.horizontal(|ui| {
            ui.label("Start from a game track:");
            let shown = if self.opts.import_dat.is_empty() {
                "(none)".to_string()
            } else {
                self.opts.import_dat.clone()
            };
            egui::ComboBox::from_id_salt("import_pick")
                .width(260.0)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(self.opts.import_dat.is_empty(), "(none)")
                        .clicked()
                    {
                        self.opts.import_dat.clear();
                    }
                    for t in &self.game_tracks {
                        let v = format!("{t}\\{t}.dat");
                        if ui.selectable_label(self.opts.import_dat == v, t).clicked() {
                            self.opts.import_dat = v;
                        }
                    }
                });
            ui.label(
                RichText::new("unpacked only into an empty track folder")
                    .small()
                    .weak(),
            );
        });
    }

    fn ui_textures(&mut self, ui: &mut egui::Ui, td: &Path) {
        ui.label(
            RichText::new("PNG / JPG / BMP / TGA -> TGA -> makemip. Unchanged images are skipped unless 'Rebuild all' is on.").weak(),
        );
        let mut remove: Option<usize> = None;
        for (i, job) in self.opts.tex_jobs.iter_mut().enumerate() {
            ui.push_id(i, |ui| {
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut job.label).desired_width(180.0));
                            ui.label("->");
                            ui.radio_value(&mut job.dest, TexDest::Trackmat, "trackmat\\");
                            ui.radio_value(&mut job.dest, TexDest::TrackRoot, "track folder");
                            if ui.small_button("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Source:");
                            rel_path_edit(ui, &mut job.src, td, "folder of images");
                        });
                        ui.horizontal_wrapped(|ui| {
                            egui::ComboBox::from_id_salt("resize")
                                .width(190.0)
                                .selected_text(job.resize.label())
                                .show_ui(ui, |ui| {
                                    for r in Resize::ALL {
                                        ui.selectable_value(&mut job.resize, r, r.label());
                                    }
                                });
                            ui.checkbox(&mut job.tile, "-tile");
                            ui.checkbox(&mut job.dxt, "-dxt");
                            ui.checkbox(&mut job.true_color, "-trueColor");
                            ui.checkbox(&mut job.trans, "-trans")
                                .on_hover_text("1-bit transparency (cutouts like fences)");
                            ui.checkbox(&mut job.force, "Rebuild all");
                        });
                    });
            });
            ui.separator();
        }
        if let Some(i) = remove {
            self.opts.tex_jobs.remove(i);
        }
        if ui.button("+ Add texture folder").clicked() {
            self.opts.tex_jobs.push(TexJob {
                label: "More textures".into(),
                src: r"src_textures\more".into(),
                ..TexJob::default()
            });
        }
    }

    fn ui_models(&mut self, ui: &mut egui::Ui, td: &Path) {
        ui.label(RichText::new("Runs make3do -s on every .psg script (scripts reference your exported .pas files).").weak());
        ui.horizontal(|ui| {
            ui.label("PSG folder:");
            rel_path_edit(ui, &mut self.opts.psg_dir, td, "(track folder)");
        });
        ui.checkbox(
            &mut self.opts.make3do_verbose,
            "Verbose (-v) — extra output to help find errors",
        );
    }

    fn ui_pack(&mut self, ui: &mut egui::Ui, td: &Path) {
        ui.label(
            RichText::new(format!(
                "Writes {0}.def listing the track folder and trackmat\\, builds {0}.dat with mklib, then verifies it.",
                self.track
            ))
            .weak(),
        );
        ui.horizontal(|ui| {
            ui.label("File types:");
            ui.add(egui::TextEdit::singleline(&mut self.opts.pack_exts).desired_width(220.0));
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.opts.compress, "Compress (-c)");
            ui.add_enabled_ui(self.opts.compress, |ui| {
                ui.label("except:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.opts.no_compress_exts)
                        .desired_width(120.0)
                        .hint_text("e.g. mip stp"),
                );
            });
        });
        let dat = td.join(format!("{}.dat", self.track));
        if let Ok(m) = std::fs::metadata(&dat) {
            ui.label(
                RichText::new(format!(
                    "Current: {} ({})",
                    dat.display(),
                    ttk::util::human_size(m.len())
                ))
                .small()
                .weak(),
            );
        }
    }

    fn ui_install(&mut self, ui: &mut egui::Ui, gd: &Path) {
        ui.label(
            RichText::new(format!(
                "Copies the .dat to {} (the previous .dat is backed up to _ttk_backup\\).",
                gd.display()
            ))
            .weak(),
        );
        ui.checkbox(
            &mut self.opts.copy_templates,
            format!(
                "Copy missing support files from '{}' (track.ini, .cam, .sim, .acd, .lp) — existing files are never overwritten",
                self.settings.template_track
            ),
        );
    }

    fn ui_stalls(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Runs mkstalls on an LP file in the installed track and replaces the stall_N lines in that track.ini section (backup kept).")
                .weak(),
        );
        let s = &mut self.opts.stalls;
        ui.checkbox(&mut s.enabled, "Include in Run all");
        egui::Grid::new("stalls_grid")
            .num_columns(4)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("LP file");
                ui.add(egui::TextEdit::singleline(&mut s.lp_file).desired_width(120.0));
                ui.label("track.ini section");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut s.section).desired_width(140.0));
                    for preset in ["pit_lane_0", "starting_grid_0"] {
                        if ui.small_button(preset).clicked() {
                            s.section = preset.into();
                        }
                    }
                });
                ui.end_row();
                ui.label("Start dlong (m)");
                ui.add(
                    egui::DragValue::new(&mut s.start_dlong)
                        .speed(1.0)
                        .max_decimals(2),
                );
                ui.label("End dlong (m)");
                ui.add(
                    egui::DragValue::new(&mut s.end_dlong)
                        .speed(1.0)
                        .max_decimals(2),
                );
                ui.end_row();
                ui.label("Stalls");
                ui.add(egui::DragValue::new(&mut s.count).range(1..=99));
                ui.label("Grid offset (m)");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut s.grid_offset)
                            .speed(0.1)
                            .max_decimals(2),
                    );
                    ui.label(
                        RichText::new("0 = pit lane · <0 pole on right · >0 pole on left")
                            .small()
                            .weak(),
                    );
                });
                ui.end_row();
            });
    }
}
