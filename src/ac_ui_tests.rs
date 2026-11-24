//! Headless test of the Assetto Corsa tab: open an AC track, press "Make NR2003 track", look at
//! the result. Runs the real panel in memory (no window, no real input), writing to a temporary
//! workspace. Skipped when AC, acdump.exe or the NR2003 template track isn't installed.

use super::*;
use eframe::egui::{accesskit::Role, Event, Modifiers, PointerButton, Pos2, Rect};

struct Widget {
    role: Role,
    label: Option<String>,
    rect: Rect,
}

struct Harness {
    ctx: egui::Context,
    view: AcView,
    settings: Settings,
    time: f64,
    widgets: Vec<Widget>,
}

impl Harness {
    fn frame(&mut self, events: Vec<Event>) {
        self.time += 0.05;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1400.0, 2600.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let Harness { ctx, view, settings, .. } = self;
        let out = ctx.run(input, |ctx| view.show(ctx, settings));
        if let Some(u) = out.platform_output.accesskit_update {
            self.widgets = u
                .nodes
                .iter()
                .filter_map(|(_, n)| {
                    let b = n.bounds()?;
                    Some(Widget {
                        role: n.role(),
                        label: n.label().or(n.value()).map(str::to_string),
                        rect: Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32)),
                    })
                })
                .collect();
        }
    }

    fn wait(&mut self, what: &str, secs: u64, done: impl Fn(&AcView) -> bool) {
        let start = std::time::Instant::now();
        while !done(&self.view) {
            assert!(start.elapsed().as_secs() < secs, "timed out waiting for {what}");
            std::thread::sleep(std::time::Duration::from_millis(30));
            self.frame(Vec::new());
        }
        for _ in 0..3 {
            self.frame(Vec::new());
        }
    }

    fn click(&mut self, label: &str) {
        let r = self
            .widgets
            .iter()
            .filter(|w| w.role == Role::Button && w.label.as_deref() == Some(label))
            .map(|w| w.rect)
            .next()
            .unwrap_or_else(|| panic!("no button {label}"));
        let p = r.center();
        let m = Modifiers::NONE;
        self.frame(vec![Event::PointerMoved(p)]);
        self.frame(vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: m }]);
        self.frame(vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: m }]);
        self.frame(Vec::new());
    }
}

#[test]
fn make_an_nr2003_track_from_mugello() {
    let Some(tracks) = ac::find_tracks_dir() else { return };
    let mugello = tracks.join("mugello");
    let mut settings = Settings::default();
    let template = settings.game_tracks().join(settings.template_track.trim());
    if !mugello.is_dir() || ac::find_acdump().is_none() || ttk::track::graph::track_file_in(&template).is_none() {
        eprintln!("skipping: mugello, acdump.exe or the template track missing");
        return;
    }
    let ws = std::env::temp_dir().join(format!("ttk_ac_ui_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    settings.workspace_dir = ws.clone();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut h = Harness { ctx, view: AcView::default(), settings, time: 0.0, widgets: Vec::new() };
    h.view.open_on_start(mugello, String::new(), false);
    h.wait("the AC track to load", 60, |v| v.loaded.is_some());
    assert_eq!(h.view.nr_name, "mugello_ac");

    h.click("Make NR2003 track");
    h.wait("the conversion", 60, |v| v.making.is_none() && (v.made.is_some() || v.make_error.is_some()));
    assert!(h.view.make_error.is_none(), "{:?}", h.view.make_error);
    let made = h.view.made.as_ref().unwrap();
    assert!(made.report.crashes.is_empty());
    assert!(made.report.worst_m < 0.5);
    let ptf = ws.join("mugello_ac").join("mugello_ac.ptf");
    assert!(ptf.is_file(), "saved to the workspace");
    assert!(ws.join("mugello_ac").join("trackmat").is_dir(), "textures copied");
    assert!(made.line.len() > 100, "the NR2003 line to draw over the AC track");

    h.click("Look at it");
    assert_eq!(h.view.take_open_request(), Some(ptf));
    let _ = std::fs::remove_dir_all(&ws);
}
