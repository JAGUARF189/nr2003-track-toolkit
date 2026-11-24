//! Track tab: editing the selected piece (shape and banking) and the track checker panel.

use super::*;
use ttk::track::check::{self, Level, Problem};
use ttk::track::edit::{self, Shape};

impl TrackView {
    /// Turn edit-function output into an undoable step (old values read from the graph).
    fn edit_changes(&mut self, label: &str, key: &str, changes: Vec<edit::Change>) {
        let Some(l) = &self.loaded else { return };
        let steps = changes
            .into_iter()
            .map(|c| {
                let old = l.graph.node(c.id).and_then(|n| n.get(&c.field)).cloned().unwrap_or(serde_json::Value::Null);
                (c.id, c.field, old, c.value)
            })
            .collect();
        self.edit(saving::EditStep::new(label, key, steps));
    }

    /// Recompute the checker's findings for the current track.
    pub(super) fn recheck(&mut self) {
        self.problems = match &self.loaded {
            Some(l) => {
                let mut p = check::check(&l.track);
                p.extend(l.file_problems.iter().cloned());
                p
            }
            None => Vec::new(),
        };
    }

    pub(super) fn crash_count(&self) -> usize {
        self.problems.iter().filter(|p| p.level == Level::Crash).count()
    }

    /// Editable facts of piece `k`, then its ground and walls.
    pub(super) fn piece_editor(&mut self, ui: &mut egui::Ui, k: usize) {
        let Some(l) = &self.loaded else { return };
        let Some(s) = l.track.segments.get(k).cloned() else { return };
        let mut action: Option<(&str, String, anyhow::Result<Vec<edit::Change>>)> = None;
        let shape_key = format!("seg{k}.shape");
        let title = if s.curve {
            format!("Piece {} — {} turn", k + 1, if s.angle > 0.0 { "left" } else { "right" })
        } else {
            format!("Piece {} — straight", k + 1)
        };
        ui.label(RichText::new(title).strong().size(15.0));

        egui::Grid::new("piece_edit").num_columns(2).spacing([12.0, 5.0]).show(ui, |ui| {
            // Straight or curve
            ui.label("Shape");
            ui.horizontal(|ui| {
                let mut curve = s.curve;
                ui.radio_value(&mut curve, false, "Straight");
                ui.radio_value(&mut curve, true, "Curve");
                if curve != s.curve {
                    let shape = if curve {
                        // keep the length: a gentle 10° left turn
                        let a = 10f64.to_radians();
                        Shape::Curve { angle: a, radius: s.length / a }
                    } else {
                        Shape::Straight { length: s.length }
                    };
                    let label = if curve { "Make a curve" } else { "Make a straight" };
                    action = Some((label, format!("seg{k}.kind"), edit::reshape(&l.track, k, shape, self.move_objects)));
                }
            });
            ui.end_row();

            ui.label("Length");
            ui.horizontal(|ui| {
                let mut len = s.length;
                let r = ui.add(egui::DragValue::new(&mut len).range(0.1..=20_000.0).speed(0.1).suffix(" m").max_decimals(3));
                ui.label(RichText::new(format!("{:.1} ft", s.length * FEET_PER_METRE)).weak());
                if r.changed() && (len - s.length).abs() > 1e-9 {
                    let shape = if s.curve {
                        // same radius, so the turn angle changes with the length
                        Shape::Curve { angle: s.angle.signum() * len / s.radius, radius: s.radius }
                    } else {
                        Shape::Straight { length: len }
                    };
                    action = Some(("Change length", shape_key.clone(), edit::reshape(&l.track, k, shape, self.move_objects)));
                }
            });
            ui.end_row();

            if s.curve {
                ui.label("Turns");
                ui.horizontal(|ui| {
                    let mut deg = s.angle.abs().to_degrees();
                    let mut left = s.angle > 0.0;
                    let r = ui.add(egui::DragValue::new(&mut deg).range(0.1..=180.0).speed(0.05).suffix("°").max_decimals(3));
                    let dir = ui.radio_value(&mut left, true, "Left").changed() | ui.radio_value(&mut left, false, "Right").changed();
                    if (r.changed() && (deg - s.angle.abs().to_degrees()).abs() > 1e-9) || dir {
                        let angle = if left { deg.to_radians() } else { -deg.to_radians() };
                        action = Some(("Change turn", shape_key.clone(), edit::reshape(&l.track, k, Shape::Curve { angle, radius: s.radius }, self.move_objects)));
                    }
                });
                ui.end_row();

                ui.label("Radius");
                ui.horizontal(|ui| {
                    let mut r_m = s.radius;
                    let r = ui.add(egui::DragValue::new(&mut r_m).range(1.0..=100_000.0).speed(0.2).suffix(" m").max_decimals(2));
                    ui.label(RichText::new(format!("{:.0} ft", s.radius * FEET_PER_METRE)).weak());
                    if r.changed() && (r_m - s.radius).abs() > 1e-9 {
                        action = Some(("Change radius", shape_key.clone(), edit::reshape(&l.track, k, Shape::Curve { angle: s.angle, radius: r_m }, self.move_objects)));
                    }
                });
                ui.end_row();
            }

            ui.label("Banking").on_hover_text(if s.curve {
                "Degrees. Positive means the outside of the turn is higher."
            } else {
                "Degrees. Positive means the right side is higher."
            });
            ui.horizontal(|ui| {
                let (b0, b1) = (edit::banking_outside(&s, 0.0), edit::banking_outside(&s, 1.0));
                let (mut n0, mut n1) = (b0, b1);
                ui.label("start");
                let r0 = ui.add(egui::DragValue::new(&mut n0).range(-60.0..=60.0).speed(0.05).suffix("°").max_decimals(2));
                ui.label("end");
                let r1 = ui.add(egui::DragValue::new(&mut n1).range(-60.0..=60.0).speed(0.05).suffix("°").max_decimals(2));
                let changed = (r0.changed() && (n0 - b0).abs() > 1e-9) || (r1.changed() && (n1 - b1).abs() > 1e-9);
                if changed {
                    action = Some(("Change banking", format!("seg{k}.bank"), edit::set_banking(&l.track, k, n0, n1)));
                }
            });
            ui.end_row();

            let h0 = s.height(0.0, 0.0);
            let h1 = s.height(1.0, 0.0);
            ui.label("Height");
            ui.label(if (h0 - h1).abs() < 0.01 { format!("{h0:.2} m") } else { format!("{h0:.2} m to {h1:.2} m") });
            ui.end_row();
        });
        ui.checkbox(&mut self.move_objects, "Move objects with the track")
            .on_hover_text("When a piece changes shape, objects beside the pieces after it move with them.");

        if let Some((label, key, result)) = action {
            match result {
                Ok(c) => {
                    let shape = key.ends_with(".shape") || key.ends_with(".kind");
                    self.edit_error = None;
                    self.edit_changes(label, &key, c);
                    if shape {
                        self.layout_changed = true;
                    }
                }
                Err(e) => self.edit_error = Some(format!("{e:#}")),
            }
        }
        if let Some(e) = &self.edit_error {
            ui.colored_label(Color32::from_rgb(230, 90, 80), e);
        }
        self.parts_panel(ui, k);
    }

    /// The checker: problems that stop the track loading, then warnings. Click one to go there.
    pub(super) fn problems_panel(&mut self, ui: &mut egui::Ui) {
        let crashes = self.crash_count();
        let warnings = self.problems.len() - crashes;
        let head = match (crashes, warnings) {
            (0, 0) => RichText::new("Check: no problems found").color(Color32::from_rgb(120, 200, 120)),
            (0, w) => RichText::new(format!("Check: {w} warning{}", if w == 1 { "" } else { "s" })).color(Color32::from_rgb(240, 190, 80)),
            (c, w) => RichText::new(format!(
                "Check: {c} problem{} will stop the track loading{}",
                if c == 1 { "" } else { "s" },
                if w > 0 { format!(", {w} warning{}", if w == 1 { "" } else { "s" }) } else { String::new() }
            ))
            .color(Color32::from_rgb(235, 95, 85)),
        };
        let mut go = None;
        egui::CollapsingHeader::new(head.strong()).id_salt("track_check").default_open(crashes > 0).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("check_scroll").max_height(160.0).show(ui, |ui| {
                for p in &self.problems {
                    let (mark, col) = match p.level {
                        Level::Crash => ("Won't load", Color32::from_rgb(235, 95, 85)),
                        Level::Warning => ("Warning", Color32::from_rgb(240, 190, 80)),
                    };
                    let text = RichText::new(format!("{mark}: {}", p.text)).color(col);
                    let r = ui.add(egui::Label::new(text).wrap().sense(Sense::click()));
                    if let Some(k) = p.piece {
                        if r.on_hover_text("Click to go to this piece").clicked() {
                            go = Some(k);
                        }
                    }
                }
            });
        });
        if let Some(k) = go {
            self.selected = Some(k);
            self.scroll_list = true;
            self.focus_segment(k);
        }
        if self.layout_changed {
            ui.label(
                RichText::new(
                    "The track's shape has changed: the AI racing lines (.lp files), cameras and pit stalls \
                     are measured along the old track and need making again.",
                )
                .weak(),
            );
        }
    }
}

/// Problems to show in the piece list: (crash, warning)
pub(super) fn marks(problems: &[Problem], k: usize) -> (bool, bool) {
    (
        problems.iter().any(|p| p.piece == Some(k) && p.level == Level::Crash),
        problems.iter().any(|p| p.piece == Some(k) && p.level == Level::Warning),
    )
}
