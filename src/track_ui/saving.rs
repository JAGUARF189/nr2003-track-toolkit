//! Track tab: edits with Undo/Redo, Save / Save as, autosave on a timer you choose, earlier
//! versions, and "save your changes?" before closing or opening another track.
//!
//! Autosave only runs when there are unsaved changes. The timer starts at the first change after
//! a save and restarts after every save. A track with no save location yet (opened from a .dat)
//! is autosaved as a safety copy in %APPDATA%\TrackToolkit\autosave\ until you choose where it goes.

use super::*;
use serde_json::{json, Value};
use std::time::{Duration, Instant, SystemTime};
use ttk::config::Settings;
use ttk::track::objects::{self, NodeEdit};
use ttk::track::save;

pub(super) struct EditStep {
    pub label: String,
    /// Steps with the same key made within a moment of each other (one drag of a number)
    /// merge into one Undo step. Empty = never merge.
    pub key: String,
    pub at: Instant,
    /// (object id, field, old value, new value)
    pub changes: Vec<(i32, String, Value, Value)>,
    /// Whole objects added, removed or replaced (adding and deleting placed objects).
    pub nodes: Vec<NodeEdit>,
    /// Only placed objects change: refresh just those, not the whole track.
    pub objects_only: bool,
}

impl EditStep {
    pub fn new(label: &str, key: &str, changes: Vec<(i32, String, Value, Value)>) -> Self {
        Self {
            label: label.to_string(),
            key: key.to_string(),
            at: Instant::now(),
            changes,
            nodes: Vec::new(),
            objects_only: false,
        }
    }

    /// A change to placed objects only.
    pub fn objects(label: &str, key: &str, changes: Vec<(i32, String, Value, Value)>, nodes: Vec<NodeEdit>) -> Self {
        Self { nodes, objects_only: true, ..Self::new(label, key, changes) }
    }
}

/// How long after the last change a change to the same thing still joins its Undo step.
const MERGE_WINDOW: Duration = Duration::from_millis(1500);

pub(super) enum Pending {
    Open { name: String, path: PathBuf },
    Close,
}

struct SaveJob {
    rx: mpsc::Receiver<Result<save::Saved, String>>,
    generation: u64,
    recovery: bool,
    auto: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Unit {
    Seconds,
    Minutes,
    Hours,
}

impl Unit {
    fn secs(self) -> f64 {
        match self {
            Unit::Seconds => 1.0,
            Unit::Minutes => 60.0,
            Unit::Hours => 3600.0,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Unit::Seconds => "seconds",
            Unit::Minutes => "minutes",
            Unit::Hours => "hours",
        }
    }
    fn for_secs(s: u64) -> Self {
        if s >= 3600 && s % 3600 == 0 {
            Unit::Hours
        } else if s >= 60 && s % 60 == 0 {
            Unit::Minutes
        } else {
            Unit::Seconds
        }
    }
}

/// Shortest and longest autosave interval (saving a big track takes about a second).
pub(super) const AUTOSAVE_MIN_SECS: u64 = 10;
pub(super) const AUTOSAVE_MAX_SECS: u64 = 7 * 24 * 3600;

pub(super) struct EditState {
    undo: Vec<EditStep>,
    redo: Vec<EditStep>,
    /// Bumped by every change; the file on disk matches `saved_generation`.
    generation: u64,
    saved_generation: u64,
    job: Option<SaveJob>,
    last_saved: Option<(Instant, String)>,
    save_error: Option<String>,
    /// Autosave timer start: first change after a save, or the last save attempt.
    clock: Instant,
    unit: Option<Unit>,
    pub pending: Option<Pending>,
    close_now: bool,
    allow_close: bool,
}

impl Default for EditState {
    fn default() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            generation: 0,
            saved_generation: 0,
            job: None,
            last_saved: None,
            save_error: None,
            clock: Instant::now(),
            unit: None,
            pending: None,
            close_now: false,
            allow_close: false,
        }
    }
}

fn ago(t: Duration) -> String {
    let s = t.as_secs();
    match s {
        0..=9 => "just now".into(),
        10..=59 => format!("{s} seconds ago"),
        60..=119 => "1 minute ago".into(),
        120..=3599 => format!("{} minutes ago", s / 60),
        3600..=7199 => "1 hour ago".into(),
        7200..=172_799 => format!("{} hours ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}

fn clock_text(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

impl TrackView {
    /// Something to undo (headless UI tests).
    #[cfg(test)]
    pub(super) fn ed_can_undo(&self) -> bool {
        !self.ed.undo.is_empty()
    }

    /// A save is running in the background (headless UI tests).
    #[cfg(test)]
    pub(super) fn saving_now(&self) -> bool {
        self.ed.job.is_some()
    }

    pub fn dirty(&self) -> bool {
        self.loaded.is_some() && self.ed.generation != self.ed.saved_generation
    }

    /// New track loaded: forget the old one's history. `restored` marks an earlier version
    /// that still has to be saved to be put back.
    pub(super) fn reset_edits(&mut self, restored: bool) {
        let unit = self.ed.unit;
        self.ed = EditState { unit, ..Default::default() };
        if restored {
            self.ed.generation = 1;
            self.ed.last_saved =
                Some((Instant::now(), "This is an earlier version. Save to put it back.".into()));
        }
    }

    /// Open another track, asking first if there are unsaved changes.
    pub(super) fn request_open(&mut self, ctx: &egui::Context, name: String, path: PathBuf) {
        if self.dirty() || self.ed.job.is_some() {
            self.ed.pending = Some(Pending::Open { name, path });
        } else {
            self.picked = Some(name.clone());
            self.open(ctx, name, path, None);
        }
    }

    // ------------------------------------------------------------------ edits
    fn apply(&mut self, step: &EditStep, forward: bool) -> bool {
        let Some(l) = &mut self.loaded else { return false };
        // Forward: objects first, then numbers. Backward: the reverse.
        if forward {
            objects::apply_nodes(&mut l.graph, &step.nodes, true);
        }
        for (id, field, old, new) in &step.changes {
            let v = if forward { new } else { old };
            let ok = l.graph.node_mut(*id).map(|n| n.set(field, v.clone()).is_ok()).unwrap_or(false);
            if !ok {
                self.error = Some(format!("couldn't change {field}"));
                return false;
            }
        }
        if !forward {
            objects::apply_nodes(&mut l.graph, &step.nodes, false);
        }
        let mut rebuilt = false;
        if step.objects_only {
            l.track.objects = model::objects_from_graph(&l.graph);
        } else if let Ok(t) = Track::from_graph(&l.graph) {
            l.track = t;
            l.mesh = mesh::build(&l.track, self.show_walls);
            rebuilt = true;
        }
        // Forget selected objects that no longer exist (an Undo of adding them).
        let ids: std::collections::HashSet<i32> = l.track.objects.iter().map(|o| o.id).collect();
        self.obj_sel.retain(|id| ids.contains(id));
        self.objects_dirty = true;
        if rebuilt {
            self.upload_mesh();
        }
        if !step.objects_only {
            self.recheck();
        }
        if !self.dirty() {
            self.ed.clock = Instant::now();
        }
        self.ed.generation += 1;
        true
    }

    pub(super) fn edit(&mut self, step: EditStep) {
        if step.nodes.is_empty() && step.changes.iter().all(|c| c.2 == c.3) {
            return;
        }
        if !self.apply(&step, true) {
            return;
        }
        self.ed.redo.clear();
        if let Some(top) = self.ed.undo.last_mut() {
            let plain = step.nodes.is_empty() && top.nodes.is_empty();
            if plain && !step.key.is_empty() && top.key == step.key && top.at.elapsed() < MERGE_WINDOW {
                // Same number still being dragged/typed: one Undo step, keeping the first old values.
                for (id, field, old, new) in step.changes {
                    match top.changes.iter_mut().find(|c| c.0 == id && c.1 == field) {
                        Some(c) => c.3 = new,
                        None => top.changes.push((id, field, old, new)),
                    }
                }
                top.at = Instant::now();
                return;
            }
        }
        self.ed.undo.push(step);
    }

    pub(super) fn undo(&mut self) {
        if let Some(step) = self.ed.undo.pop() {
            if self.apply(&step, false) {
                self.ed.redo.push(step);
            }
        }
    }

    pub(super) fn redo(&mut self) {
        if let Some(step) = self.ed.redo.pop() {
            if self.apply(&step, true) {
                self.ed.undo.push(step);
            }
        }
    }

    /// Day/night. Lighting values follow the stock files (0.2 for day, 0.5 for night) unless
    /// the track uses its own values.
    pub(super) fn set_night(&mut self, night: bool) {
        let Some(l) = &self.loaded else { return };
        let root = l.graph.root();
        let mut changes = vec![(root.id, "night".to_string(), json!(root.i("night")), json!(night as i64))];
        let (from, to) = if night { (0.2, 0.5) } else { (0.5, 0.2) };
        for k in 0..3 {
            let f = format!("light{k}");
            let v = root.f(&f);
            if (v - from).abs() < 1e-6 {
                changes.push((root.id, f, root.get(&format!("light{k}")).cloned().unwrap_or(json!(v)), json!(to)));
            }
        }
        let label = if night { "Night lighting" } else { "Day lighting" };
        self.edit(EditStep::new(label, "", changes));
    }

    // ------------------------------------------------------------------ saving
    /// Start saving in the background. `auto`: from the timer. `save_as`: always ask where.
    /// Returns false if nothing was started (no track, already saving, or the dialog was cancelled).
    pub(super) fn start_save(&mut self, auto: bool, save_as: bool) -> bool {
        if self.ed.job.is_some() {
            return false;
        }
        let Some(l) = &self.loaded else { return false };
        if !auto && !save_as && l.save_path.is_some() && !self.dirty() {
            // Nothing to write; don't churn a backup of an identical file.
            self.ed.save_error = None;
            self.ed.last_saved = Some((Instant::now(), "Already saved; nothing has changed.".into()));
            return false;
        }
        let (dest, recovery) = match (&l.save_path, save_as) {
            (Some(p), false) => (p.clone(), false),
            _ if auto => (Settings::recovery_dir().join(format!("{}.ptf", l.name)), true),
            _ => {
                let start_dir = l
                    .save_path
                    .as_ref()
                    .and_then(|p| p.parent().map(Path::to_path_buf))
                    .or_else(|| self.workspace.clone().map(|w| w.join(&l.name)).filter(|d| d.is_dir()))
                    .or_else(|| self.workspace.clone());
                let mut dlg = rfd::FileDialog::new()
                    .set_title("Save the track as")
                    .add_filter("Track", &["ptf"])
                    .set_file_name(format!("{}.ptf", l.name));
                if let Some(d) = start_dir {
                    dlg = dlg.set_directory(d);
                }
                match dlg.save_file() {
                    Some(p) => (p.with_extension("ptf"), false),
                    None => return false,
                }
            }
        };
        let json = match l.graph.to_json() {
            Ok(j) => j,
            Err(e) => {
                self.ed.save_error = Some(format!("{e:#}"));
                return false;
            }
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(save::save_json(&json, &dest).map_err(|e| format!("{e:#}")));
        });
        self.ed.job = Some(SaveJob { rx, generation: self.ed.generation, recovery, auto });
        true
    }

    fn poll_save(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.ed.job else { return };
        let r = match job.rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("saving stopped unexpectedly".into()),
        };
        let job = self.ed.job.take().unwrap();
        let now = Instant::now();
        self.ed.clock = now;
        match r {
            Ok(saved) => {
                self.ed.save_error = None;
                let what = if job.auto { "Autosaved" } else { "Saved" };
                let msg = if job.recovery {
                    format!(
                        "Autosaved a safety copy to {}. Use Save to choose where the track goes.",
                        saved.path.display()
                    )
                } else {
                    self.ed.saved_generation = job.generation;
                    if let Some(l) = &mut self.loaded {
                        l.save_path = Some(saved.path.clone());
                        l.folder = saved.path.parent().map(Path::to_path_buf);
                        l.source = saved.path.display().to_string();
                    }
                    self.save_locks();
                    format!("{what} to {}", saved.path.display())
                };
                self.ed.last_saved = Some((now, msg));
            }
            Err(e) => self.ed.save_error = Some(e),
        }
        // Carry on with whatever was waiting for the save.
        if !self.dirty() {
            match self.ed.pending.take() {
                Some(Pending::Close) => {
                    self.ed.allow_close = true;
                    self.ed.close_now = true;
                }
                Some(Pending::Open { name, path }) => {
                    self.picked = Some(name.clone());
                    self.open(ctx, name, path, None);
                }
                None => {}
            }
        }
    }

    /// Called every frame from the app, on every tab: finishes saves, runs autosave and shows
    /// the "save your changes?" question.
    pub fn tick(&mut self, ctx: &egui::Context, settings: &Settings) {
        self.poll_save(ctx);
        if self.ed.job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if self.dirty() && settings.autosave {
            let every = Duration::from_secs(settings.autosave_secs.clamp(AUTOSAVE_MIN_SECS, AUTOSAVE_MAX_SECS));
            let waited = self.ed.clock.elapsed();
            if waited >= every {
                self.start_save(true, false);
                ctx.request_repaint_after(Duration::from_millis(100));
            } else {
                // wake up for the countdown (once a second) and for the save itself
                ctx.request_repaint_after((every - waited).min(Duration::from_secs(1)));
            }
        }
        self.dialogs(ctx);
    }

    /// The window is closing. Returns true if the close must be cancelled to ask first.
    pub fn on_close_requested(&mut self) -> bool {
        if (self.dirty() || self.ed.job.is_some()) && !self.ed.allow_close {
            self.ed.pending = Some(Pending::Close);
            true
        } else {
            false
        }
    }

    /// True once, when the app should close now (after "save your changes?").
    pub fn take_close(&mut self) -> bool {
        std::mem::take(&mut self.ed.close_now)
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.ed.pending else { return };
        let name = self.loaded.as_ref().map(|l| l.name.clone()).unwrap_or_default();
        let question = match p {
            Pending::Close => format!("Save your changes to {name} before closing?"),
            Pending::Open { .. } => format!("Save your changes to {name} before opening another track?"),
        };
        let leave = match p {
            Pending::Close => "Close without saving",
            Pending::Open { .. } => "Don't save",
        };
        let mut choice = None;
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(RichText::new(question).size(16.0));
                if let Some(e) = &self.ed.save_error {
                    ui.colored_label(Color32::from_rgb(230, 90, 80), format!("The last save didn't work: {e}"));
                }
                ui.add_space(8.0);
                if self.ed.job.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Saving…");
                    });
                    return;
                }
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Save").strong().size(16.0))).clicked() {
                        choice = Some(0);
                    }
                    if ui.button(RichText::new(leave).size(16.0)).clicked() {
                        choice = Some(1);
                    }
                    if ui.button(RichText::new("Keep working").size(16.0)).clicked() {
                        choice = Some(2);
                    }
                });
            });
        match choice {
            Some(0) => {
                // Continues in poll_save once the file is safely written. If the save-as
                // dialog is cancelled nothing starts and the question stays up.
                self.start_save(false, false);
            }
            Some(1) => match self.ed.pending.take() {
                Some(Pending::Close) => {
                    self.ed.allow_close = true;
                    self.ed.close_now = true;
                }
                Some(Pending::Open { name, path }) => {
                    self.picked = Some(name.clone());
                    self.open(ctx, name, path, None);
                }
                None => {}
            },
            Some(2) => self.ed.pending = None,
            _ => {}
        }
    }

    /// Ctrl+S, Ctrl+Z, Ctrl+Y / Ctrl+Shift+Z while the Track tab is showing.
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let (save, undo, redo, redo2) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::S)),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z)),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Y)),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)),
            )
        });
        if save {
            self.start_save(false, false);
        }
        if undo {
            self.undo();
        }
        if redo || redo2 {
            self.redo();
        }
    }

    /// Save buttons, status, autosave setting and earlier versions.
    pub(super) fn save_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, settings: &mut Settings) {
        let saving = self.ed.job.is_some();
        if self.crash_count() > 0 {
            ui.colored_label(
                Color32::from_rgb(235, 95, 85),
                "This track has problems that will stop it loading (see Check below). It still saves, so you can fix them later.",
            );
        }
        ui.horizontal(|ui| {
            let save = egui::Button::new(RichText::new("Save").strong().size(15.0));
            if ui.add_enabled(!saving, save).on_hover_text("Ctrl+S").clicked() {
                self.start_save(false, false);
            }
            if ui.add_enabled(!saving, egui::Button::new("Save as…")).clicked() {
                self.start_save(false, true);
            }
            ui.separator();
            let undo_tip = self.ed.undo.last().map(|s| format!("Undo: {} (Ctrl+Z)", s.label));
            if ui
                .add_enabled(undo_tip.is_some(), egui::Button::new("Undo"))
                .on_hover_text(undo_tip.unwrap_or_default())
                .clicked()
            {
                self.undo();
            }
            let redo_tip = self.ed.redo.last().map(|s| format!("Redo: {} (Ctrl+Y)", s.label));
            if ui
                .add_enabled(redo_tip.is_some(), egui::Button::new("Redo"))
                .on_hover_text(redo_tip.unwrap_or_default())
                .clicked()
            {
                self.redo();
            }
        });

        // Status
        let where_to = self
            .loaded
            .as_ref()
            .and_then(|l| l.save_path.as_ref())
            .map(|p| format!("Saves to {}", p.display()))
            .unwrap_or_else(|| "Not saved anywhere yet: Save will ask where it goes.".into());
        let status = ui.horizontal_wrapped(|ui| {
            if saving {
                ui.spinner();
                ui.label("Saving…");
            } else if let Some(e) = &self.ed.save_error {
                ui.colored_label(Color32::from_rgb(230, 90, 80), format!("Couldn't save: {e}"));
            } else if self.dirty() {
                let mut t = String::from("Unsaved changes");
                if settings.autosave {
                    let every = Duration::from_secs(settings.autosave_secs.clamp(AUTOSAVE_MIN_SECS, AUTOSAVE_MAX_SECS));
                    let left = every.saturating_sub(self.ed.clock.elapsed());
                    t += &format!("  ·  autosave in {}", clock_text(left));
                }
                ui.colored_label(Color32::from_rgb(240, 190, 80), t);
                if let Some((_, msg)) = &self.ed.last_saved {
                    // Short here; the full message (with the folder) is in the hover text.
                    if msg.starts_with("Autosaved a safety copy") {
                        ui.label(RichText::new("Safety copy autosaved").weak());
                    } else if msg.starts_with("This is an earlier") {
                        ui.label(RichText::new("Earlier version: Save to put it back").weak());
                    }
                }
            } else if let Some((at, _)) = &self.ed.last_saved {
                ui.colored_label(Color32::from_rgb(120, 200, 120), format!("All changes saved ({})", ago(at.elapsed())));
            } else {
                ui.label(RichText::new("No changes yet").weak());
            }
        });
        status.response.on_hover_text(match &self.ed.last_saved {
            Some((_, m)) => format!("{m}\n{where_to}"),
            None => where_to,
        });
        if self.dirty() || saving {
            ctx.request_repaint_after(Duration::from_secs(1));
        }

        // Autosave interval
        ui.horizontal(|ui| {
            let mut changed = ui.checkbox(&mut settings.autosave, "Save automatically every").changed();
            let mut unit = self.ed.unit.unwrap_or_else(|| Unit::for_secs(settings.autosave_secs));
            let mut v = settings.autosave_secs as f64 / unit.secs();
            let lo = AUTOSAVE_MIN_SECS as f64 / unit.secs();
            let hi = AUTOSAVE_MAX_SECS as f64 / unit.secs();
            ui.add_enabled_ui(settings.autosave, |ui| {
                let dv = egui::DragValue::new(&mut v)
                    .range(lo..=hi)
                    .speed(0.2)
                    .custom_formatter(|n, _| {
                        if n.fract() == 0.0 { format!("{n:.0}") } else { format!("{:.2}", n).trim_end_matches('0').to_string() }
                    });
                changed |= ui.add(dv).on_hover_text("Any amount: type a number or drag").changed();
                egui::ComboBox::from_id_salt("autosave_unit").selected_text(unit.name()).width(80.0).show_ui(ui, |ui| {
                    for u in [Unit::Seconds, Unit::Minutes, Unit::Hours] {
                        if ui.selectable_value(&mut unit, u, u.name()).changed() {
                            changed = true;
                        }
                    }
                });
            });
            if changed {
                self.ed.unit = Some(unit);
                settings.autosave_secs =
                    ((v * unit.secs()).round() as u64).clamp(AUTOSAVE_MIN_SECS, AUTOSAVE_MAX_SECS);
                let _ = settings.save();
            }
        });

        // Earlier versions (backups made by each save)
        if let Some(dest) = self.loaded.as_ref().and_then(|l| l.save_path.clone()) {
            egui::CollapsingHeader::new("Earlier versions").id_salt("earlier_versions").show(ui, |ui| {
                let list = save::backups(&dest);
                if list.is_empty() {
                    ui.label(RichText::new("None yet. Every save keeps the version before it.").weak());
                }
                let mut open = None;
                for (path, when) in list.iter().take(save::TRACK_BACKUPS_KEPT) {
                    ui.horizontal(|ui| {
                        let age = SystemTime::now().duration_since(*when).unwrap_or_default();
                        ui.label(format!("Saved {}", ago(age))).on_hover_text(path.display().to_string());
                        if ui.small_button("Open").clicked() {
                            open = Some(path.clone());
                        }
                    });
                }
                if let Some(backup) = open {
                    if self.dirty() {
                        self.ed.save_error =
                            Some("Save or undo your changes before opening an earlier version.".into());
                    } else if let Some(l) = &self.loaded {
                        let name = l.name.clone();
                        self.open(ctx, name, backup, Some(dest.clone()));
                    }
                }
            });
        }
    }
}
