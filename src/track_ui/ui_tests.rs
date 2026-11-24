//! Headless tests of the Track tab's editors: the real panels run in memory, widgets are found
//! by name through egui's accessibility tree, and clicks and typing are fed in as input events.
//! No window opens and no real mouse or keyboard input is sent.
//!
//! Run with `cargo test --release --bin track-toolkit`. Skipped when the test track or
//! ptfjson.exe isn't there.

use super::*;
use eframe::egui::{accesskit::Role, Event, Key, Modifiers, PointerButton, Pos2, Rect};

struct Widget {
    role: Role,
    label: Option<String>,
    number: Option<f64>,
    rect: Rect,
}

struct Harness {
    ctx: egui::Context,
    view: TrackView,
    settings: Settings,
    tracks: Vec<(String, PathBuf)>,
    workspace: PathBuf,
    time: f64,
    widgets: Vec<Widget>,
}

impl Harness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Self {
            ctx,
            view: TrackView::default(),
            settings: Settings::default(),
            tracks: Vec::new(),
            workspace: std::env::temp_dir(),
            time: 0.0,
            widgets: Vec::new(),
        }
    }

    /// One frame with these input events.
    fn frame(&mut self, events: Vec<Event>, modifiers: Modifiers) {
        self.time += 0.05;
        let input = egui::RawInput {
            // tall, so the side panel's scroll areas show everything
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1400.0, 2600.0))),
            time: Some(self.time),
            modifiers,
            events,
            ..Default::default()
        };
        let Harness { ctx, view, settings, tracks, workspace, .. } = self;
        let env = TrackEnv { game_tracks: tracks, workspace };
        let out = ctx.run(input, |ctx| {
            view.tick(ctx, settings);
            view.show(ctx, &env, settings);
        });
        if let Some(u) = out.platform_output.accesskit_update {
            self.widgets = u
                .nodes
                .iter()
                .filter_map(|(_, n)| {
                    let b = n.bounds()?;
                    Some(Widget {
                        role: n.role(),
                        // buttons carry their text as a label, plain text as a value
                        label: n.label().or(n.value()).map(str::to_string),
                        number: n.numeric_value(),
                        rect: Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32)),
                    })
                })
                .collect();
        }
    }

    fn idle(&mut self, frames: usize) {
        for _ in 0..frames {
            self.frame(Vec::new(), Modifiers::NONE);
        }
    }

    /// Run frames until `done` (background loading/saving), up to 30 s.
    fn wait(&mut self, what: &str, done: impl Fn(&TrackView) -> bool) {
        let start = std::time::Instant::now();
        while !done(&self.view) {
            assert!(start.elapsed().as_secs() < 30, "timed out waiting for {what}");
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.idle(1);
        }
        self.idle(3);
    }

    fn click_at(&mut self, p: Pos2) {
        let m = Modifiers::NONE;
        self.frame(vec![Event::PointerMoved(p)], m);
        self.frame(vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: m }], m);
        self.frame(vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: m }], m);
        self.idle(2);
    }

    fn find(&self, what: &str, pick: impl Fn(&Widget) -> bool) -> Rect {
        let found: Vec<&Widget> = self.widgets.iter().filter(|w| pick(w)).collect();
        if found.is_empty() {
            let seen: Vec<String> = self
                .widgets
                .iter()
                .filter(|w| w.role != Role::Label && w.role != Role::GenericContainer)
                .map(|w| format!("{:?} {:?} {:?}", w.role, w.label, w.number))
                .collect();
            panic!("no widget: {what}
on screen: {seen:#?}");
        }
        // first in reading order
        found.iter().min_by(|a, b| (a.rect.top(), a.rect.left()).partial_cmp(&(b.rect.top(), b.rect.left())).unwrap()).unwrap().rect
    }

    /// Click the widget with exactly this label (button, selectable label, combo box...).
    fn click(&mut self, label: &str) {
        let r = self.find(label, |w| w.label.as_deref() == Some(label) && w.role != Role::Label);
        self.click_at(r.center());
    }

    /// Open the drop-down list on the same row as the text `beside` (e.g. "Type").
    fn open_combo(&mut self, beside: &str) {
        let row = self.find(beside, |w| matches!(w.role, Role::Label | Role::TextRun) && w.label.as_deref() == Some(beside));
        let r = self.find(&format!("drop-down beside {beside}"), |w| {
            w.role == Role::ComboBox && w.rect.center().y > row.top() - 4.0 && w.rect.center().y < row.bottom() + 4.0
        });
        self.click_at(r.center());
    }

    /// Click the number box currently showing about `value`, select all and type `text`.
    fn type_into(&mut self, value: f64, text: &str) {
        let r = self.find(&format!("number box showing {value}"), |w| {
            w.role == Role::SpinButton && w.number.is_some_and(|n| (n - value).abs() < 0.005)
        });
        self.click_at(r.center());
        let ctrl = Modifiers::COMMAND;
        let key = |key: Key, pressed: bool, modifiers: Modifiers| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
        self.frame(vec![key(Key::A, true, ctrl), key(Key::A, false, ctrl)], ctrl);
        self.frame(vec![Event::Text(text.to_string())], Modifiers::NONE);
        self.frame(vec![key(Key::Enter, true, Modifiers::NONE), key(Key::Enter, false, Modifiers::NONE)], Modifiers::NONE);
        self.idle(3);
    }

    fn track(&self) -> &Track {
        &self.view.loaded.as_ref().expect("a track is loaded").track
    }

    /// Open a copy of martinsville with piece `piece` (1-based) selected.
    fn open_copy(name: &str, piece: usize, part: Option<(bool, usize)>) -> Option<(Self, PathBuf)> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("_test/ptfs/martinsville/martinsville.ptf");
        if !src.is_file() || graph::find_ptfjson().is_none() {
            eprintln!("skipping: {} or ptfjson.exe missing", src.display());
            return None;
        }
        let dir = std::env::temp_dir().join(format!("ttk_ui_test_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("martinsville.ptf");
        std::fs::copy(&src, &file).unwrap();
        let mut h = Self::new();
        let ctx = h.ctx.clone();
        h.view.open_path(&ctx, file.clone(), false, Some(piece - 1));
        if let Some((wall, n)) = part {
            h.view.pick_on_load(wall, n);
        }
        h.wait("the track to load", |v| v.loaded.is_some() && v.selected == Some(piece - 1));
        Some((h, file))
    }

    fn save(&mut self) {
        self.click("Save");
        self.wait("saving", |v| !v.saving_now() && !v.dirty());
    }
}

fn reload(file: &Path) -> (graph::Graph, Track) {
    let g = graph::read_ptf_file(file).unwrap();
    let t = Track::from_graph(&g).unwrap();
    (g, t)
}

#[test]
fn shape_editor_typing_undo_and_save() {
    let Some((mut h, file)) = Harness::open_copy("shape", 15, None) else { return };
    let total = h.track().length;
    let len = h.track().segments[14].length;

    // Type a new length for the back straight: 10 m longer, the rest of the track moves along.
    h.type_into(len, &format!("{:.3}", len + 10.0));
    assert!((h.track().segments[14].length - (len + 10.0)).abs() < 1e-3, "length {}", h.track().segments[14].length);
    assert!((h.track().length - (total + 10.0)).abs() < 1e-3);
    assert!(h.view.layout_changed);
    assert!(h.view.problems.iter().any(|p| p.text.contains("doesn't close")), "the checker sees the gap");

    // Undo puts it back.
    h.click("Undo");
    assert!((h.track().segments[14].length - len).abs() < 1e-6);
    assert!((h.track().length - total).abs() < 1e-6);

    // Piece 9 (a left turn): pick it in the list, then type banking and radius.
    let row = piece_line(&h.track().segments[8]);
    h.click(&row);
    assert_eq!(h.view.selected, Some(8));
    let s = h.track().segments[8].clone();
    let bank0 = ttk::track::edit::banking_outside(&s, 0.0);
    h.type_into((bank0 * 100.0).round() / 100.0, "14");
    let s2 = h.track().segments[8].clone();
    assert!((ttk::track::edit::banking_outside(&s2, 0.0) - 14.0).abs() < 0.01, "banking {}", ttk::track::edit::banking_outside(&s2, 0.0));
    h.type_into((s.radius * 100.0).round() / 100.0, "60");
    assert!((h.track().segments[8].radius - 60.0).abs() < 1e-3, "radius {}", h.track().segments[8].radius);

    // Save, then read the file back from disk.
    h.save();
    let (_, t) = reload(&file);
    assert!((t.segments[8].radius - 60.0).abs() < 1e-3);
    assert!((ttk::track::edit::banking_outside(&t.segments[8], 0.0) - 14.0).abs() < 0.01);
    assert!(file.parent().unwrap().join("_ttk_backup").is_dir(), "a backup was made");
}

#[test]
fn wall_and_strip_editor() {
    let Some((mut h, file)) = Harness::open_copy("walls", 2, Some((true, 1))) else { return };
    let k = 1;
    let wall = h.view.part.expect("wall 1 picked");
    let w0 = h.track().segments[k].ws.iter().find(|w| w.id == wall).unwrap().clone();
    let n_walls = h.track().segments[k].ws.len();

    // Type: pick "Tire wall" from the list.
    h.open_combo("Type");
    h.click("Tire wall");
    let w = |h: &Harness| h.track().segments[k].ws.iter().find(|x| x.id == wall).unwrap().clone();
    assert_eq!(w(&h).wall_type, 2055);

    // Height at the start: type 2.
    h.type_into(w0.height[0], "2");
    assert!((w(&h).height[0] - 2.0).abs() < 1e-9, "height {:?}", w(&h).height);

    // Add a wall on the right, then delete it again.
    h.click("Add a wall on the right");
    assert_eq!(h.track().segments[k].ws.len(), n_walls + 1);
    let added = h.view.part.unwrap();
    assert_ne!(added, wall);
    h.click("Delete this wall");
    assert_eq!(h.track().segments[k].ws.len(), n_walls);
    assert!(h.view.part.is_none());

    // Ground: pick the first strip in the list.
    let s = h.track().segments[k].clone();
    let (a, b, surf) = s.strips(0.5)[0];
    h.click(&format!("{} {:.1} m", surface_name(surf), b - a));
    let strip = h.view.part.expect("strip picked");
    assert!(s.fs.iter().any(|f| f.id == strip));
    let n_strips = s.fs.len();

    // Surface: Grass.
    h.open_combo("Surface");
    h.click("Grass");
    let f = |h: &Harness| h.track().segments[k].fs.iter().find(|x| x.id == strip).cloned();
    assert_eq!(f(&h).unwrap().surface, 4);

    // Split in two, then remove the new half.
    h.click("Split in two");
    assert_eq!(h.track().segments[k].fs.len(), n_strips + 1);
    h.click("Remove");
    assert_eq!(h.track().segments[k].fs.len(), n_strips);
    // pick the strip again (Remove clears the pick)
    let s = h.track().segments[k].clone();
    let (a, b, surf) = s.strips(0.5)[0];
    h.click(&format!("{} {:.1} m", surface_name(surf), b - a));
    assert_eq!(h.view.part, Some(strip));

    // Texture: choose a different one from the list.
    let old = f(&h).unwrap().tex.unwrap().file;
    h.open_combo("Texture");
    // the top-most other entry, so it's in the visible part of the scrolling list
    let other = h
        .widgets
        .iter()
        .filter(|w| w.role == Role::Button)
        .filter(|w| w.label.as_deref().is_some_and(|l| l.to_ascii_lowercase().starts_with("trackmat\\") && !l.eq_ignore_ascii_case(&old)))
        .min_by(|a, b| a.rect.top().total_cmp(&b.rect.top()))
        .and_then(|w| w.label.clone())
        .expect("another texture in the list");
    h.click(&other);
    assert_eq!(f(&h).unwrap().tex.unwrap().file, other);

    // Copy to the next piece (the suggested range is piece 3 to 3).
    assert_eq!(h.view.copy_to, (3, 3));
    h.click("Copy");
    let d = 0.5 * (a + b);
    let s3 = h.track().segments[2].clone();
    let under = s3.fs.iter().filter(|x| 0.5 * (x.dlat[0] + x.dlat[1]) <= d).max_by(|p, q| (p.dlat[0] + p.dlat[1]).total_cmp(&(q.dlat[0] + q.dlat[1]))).unwrap();
    assert_eq!(under.surface, 4, "piece 3's strip took the surface");
    assert_eq!(under.tex.as_ref().unwrap().file, other, "and the texture");

    // Save and read back.
    h.save();
    let (_, t) = reload(&file);
    let sw = t.segments[k].ws.iter().find(|x| (x.dlat[0] - w0.dlat[0]).abs() < 1e-4 && x.wall_type == 2055);
    assert!(sw.is_some_and(|x| (x.height[0] - 2.0).abs() < 1e-6), "wall saved");
    assert!(t.segments[k].fs.iter().any(|x| x.surface == 4 && x.tex.as_ref().is_some_and(|tx| tx.file == other)), "strip saved");

    // Undo all the way back to the file as loaded.
    while h.view.ed_can_undo() {
        h.click("Undo");
    }
    let t0 = reload(Path::new(env!("CARGO_MANIFEST_DIR")).join("_test/ptfs/martinsville/martinsville.ptf").as_path()).1;
    let a0: Vec<(i64, f64)> = t0.segments[k].ws.iter().map(|w| (w.wall_type, w.height[0])).collect();
    let a1: Vec<(i64, f64)> = h.track().segments[k].ws.iter().map(|w| (w.wall_type, w.height[0])).collect();
    assert_eq!(a0, a1);
}

#[test]
fn clicking_the_map_picks_the_wall_or_ground_there() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("_test/ptfs/martinsville/martinsville.ptf");
    if !src.is_file() || graph::find_ptfjson().is_none() {
        return;
    }
    let (_, t) = reload(&src);
    let s = &t.segments[1];
    for w in &s.ws {
        let p = s.point(0.5, 0.5 * (w.dlat[0] + w.dlat[1]));
        assert_eq!(parts::pick_part(s, p[0], p[1], 0.05), Some(w.id), "wall at {:?}", w.dlat);
    }
    // the middle of the widest strip, away from any wall
    let (a, b, _) = s.strips(0.5).into_iter().max_by(|x, y| (x.1 - x.0).total_cmp(&(y.1 - y.0))).unwrap();
    let p = s.point(0.5, 0.5 * (a + b));
    let id = parts::pick_part(s, p[0], p[1], 0.05).unwrap();
    assert!(s.fs.iter().any(|f| f.id == id && (0.5 * (f.dlat[0] + f.dlat[1]) - a).abs() < 1e-6));
}
