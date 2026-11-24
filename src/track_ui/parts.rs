//! Track tab: the walls and ground strips of the selected piece. Pick one (in the list, or by
//! clicking it on the map) to change its type, size, position and texture, add or remove it,
//! or copy it along the track.

use super::*;
use ttk::track::edit::Change;
use ttk::track::objects::NodeEdit;
use ttk::track::surface::{self, Side};

/// Show a sideways distance in plain words: "3.20 m right" / "1.50 m left".
fn side_text(v: f64) -> String {
    if v.abs() < 0.0005 {
        "0 m (centre)".into()
    } else if v < 0.0 {
        format!("{:.2} m right", -v)
    } else {
        format!("{v:.2} m left")
    }
}

/// Read "3.2 right", "3.2 r", "-3.2" or "1.5 left" back into a dlat.
fn side_parse(s: &str) -> Option<f64> {
    let l = s.to_ascii_lowercase();
    let num: String = l.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
    let v: f64 = num.parse().ok()?;
    Some(if l.contains('r') && v > 0.0 { -v } else { v })
}

fn side_value<'a>(v: &'a mut f64, speed: f64) -> egui::DragValue<'a> {
    egui::DragValue::new(v)
        .speed(speed)
        .range(-1000.0..=1000.0)
        .custom_formatter(|v, _| side_text(v))
        .custom_parser(side_parse)
}

fn lerp2(a: [f64; 2], t: f64) -> f64 {
    a[0] + (a[1] - a[0]) * t
}

/// The wall or ground strip of piece `s` under world point (x, y), if any. `slack` is how far
/// off (metres) still counts.
pub(super) fn pick_part(s: &model::Segment, x: f64, y: f64, slack: f64) -> Option<i32> {
    let (t, d) = s.locate(x, y);
    let t = t.clamp(0.0, 1.0);
    // Walls under the point (or within `slack`); where walls overlap (a thin fence standing in a
    // thicker wall), the one whose middle is nearest wins.
    let wall = s
        .ws
        .iter()
        .map(|w| {
            let off = (d - lerp2(w.dlat, t)).abs();
            ((off - 0.5 * lerp2(w.width, t)).max(0.0), off, w.id)
        })
        .filter(|(gap, _, _)| *gap <= slack)
        .min_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).unwrap());
    if let Some((_, _, id)) = wall {
        return Some(id);
    }
    if d > s.max_dlat(t) + slack || d < s.min_dlat(t) - slack {
        return None;
    }
    s.fs
        .iter()
        .filter(|f| lerp2(f.dlat, t) <= d)
        .max_by(|a, b| lerp2(a.dlat, t).total_cmp(&lerp2(b.dlat, t)))
        .or_else(|| s.fs.iter().min_by(|a, b| lerp2(a.dlat, t).total_cmp(&lerp2(b.dlat, t))))
        .map(|f| f.id)
}

/// Outline of a wall or strip on piece `s` (world x, y, z), for drawing the selection.
pub(super) fn part_outline(s: &model::Segment, id: i32) -> Vec<[f64; 3]> {
    let n = 24;
    let ts: Vec<f64> = (0..=n).map(|k| k as f64 / n as f64).collect();
    let (right, left): (Box<dyn Fn(f64) -> f64>, Box<dyn Fn(f64) -> f64>) = if let Some(w) = s.ws.iter().find(|w| w.id == id) {
        let w = w.clone();
        let w2 = w.clone();
        (
            Box::new(move |t| lerp2(w.dlat, t) - 0.5 * lerp2(w.width, t)),
            Box::new(move |t| lerp2(w2.dlat, t) + 0.5 * lerp2(w2.width, t)),
        )
    } else if let Some(f) = s.fs.iter().find(|f| f.id == id) {
        let (f, s2) = (f.clone(), s.clone());
        let f2 = f.clone();
        (
            Box::new(move |t| lerp2(f.dlat, t)),
            Box::new(move |t| {
                let d = lerp2(f2.dlat, t);
                let next = s2.fs.iter().map(|x| lerp2(x.dlat, t)).filter(|&x| x > d + 1e-6).fold(f64::MAX, f64::min);
                if next == f64::MAX { s2.max_dlat(t) } else { next }
            }),
        )
    } else {
        return Vec::new();
    };
    let mut pts = Vec::new();
    for &t in &ts {
        let d = right(t);
        let p = s.point(t, d);
        pts.push([p[0], p[1], s.height(t, d) + 0.05]);
    }
    for &t in ts.iter().rev() {
        let d = left(t);
        let p = s.point(t, d);
        pts.push([p[0], p[1], s.height(t, d) + 0.05]);
    }
    pts
}

impl TrackView {
    /// Number changes to a wall or strip as one undoable step (old values read from the graph).
    fn part_changes(&mut self, label: &str, key: &str, changes: Vec<Change>, nodes: Vec<NodeEdit>) {
        let Some(l) = &self.loaded else { return };
        let steps = changes
            .into_iter()
            .map(|c| {
                let old = l.graph.node(c.id).and_then(|n| n.get(&c.field)).cloned().unwrap_or(serde_json::Value::Null);
                (c.id, c.field, old, c.value)
            })
            .collect();
        let mut step = saving::EditStep::new(label, key, steps);
        step.nodes = nodes;
        self.edit_error = None;
        self.part_note = None;
        self.edit(step);
    }

    /// Walls and ground strips of piece `k`, and the editor for the one picked.
    pub(super) fn parts_panel(&mut self, ui: &mut egui::Ui, k: usize) {
        let Some(l) = &self.loaded else { return };
        let s = l.track.segments[k].clone();
        if self.part.is_some_and(|id| !s.ws.iter().any(|w| w.id == id) && !s.fs.iter().any(|f| f.id == id)) {
            self.part = None;
        }
        let mut pick = None;

        // Ground strips, right to left
        ui.add_space(4.0);
        ui.label(RichText::new("Ground, right to left (click one to change it)").weak());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (a, b, surf) in s.strips(0.5) {
                let Some(f) = s.fs.iter().find(|f| (lerp2(f.dlat, 0.5) - a).abs() < 1e-6) else { continue };
                let text = format!("{} {:.1} m", surface_name(surf), b - a);
                if ui.selectable_label(self.part == Some(f.id), text).clicked() {
                    pick = Some(f.id);
                }
            }
        });

        // Walls, right to left
        ui.horizontal(|ui| {
            ui.label(RichText::new("Walls").weak());
            if ui.small_button("Add a wall on the right").clicked() {
                self.add_wall(k, Side::Right);
            }
            if ui.small_button("on the left").clicked() {
                self.add_wall(k, Side::Left);
            }
        });
        let mut ws: Vec<&model::WSec> = s.ws.iter().collect();
        ws.sort_by(|a, b| a.dlat[0].total_cmp(&b.dlat[0]));
        if ws.is_empty() {
            ui.label(RichText::new("No walls on this piece.").weak());
        }
        for w in ws {
            let text = format!(
                "{}  —  {}, {:.2} m tall",
                wall_name(w.wall_type),
                side_text(0.5 * (w.dlat[0] + w.dlat[1])),
                0.5 * (w.height[0] + w.height[1])
            );
            if ui.selectable_label(self.part == Some(w.id), text).on_hover_text(format!("Sandbox wall type {}", w.wall_type)).clicked() {
                pick = Some(w.id);
            }
        }
        if let Some(id) = pick {
            self.part = if self.part == Some(id) { None } else { Some(id) };
        }
        if self.copy_for != self.part {
            // suggest the next piece
            let n = s.index + 2;
            let last = self.loaded.as_ref().map_or(1, |l| l.track.segments.len());
            self.copy_to = (n.min(last), n.min(last));
            self.copy_for = self.part;
        }

        if let Some(n) = &self.part_note {
            ui.label(RichText::new(n).color(Color32::from_rgb(200, 210, 230)));
        }
        let Some(id) = self.part else { return };
        ui.add_space(4.0);
        let r = egui::Frame::group(ui.style()).show(ui, |ui| {
            if s.ws.iter().any(|w| w.id == id) {
                self.wall_editor(ui, k, id);
            } else {
                self.strip_editor(ui, k, id);
            }
        });
        if std::mem::take(&mut self.scroll_part) {
            r.response.scroll_to_me(Some(egui::Align::Min));
        }
    }

    fn add_wall(&mut self, k: usize, side: Side) {
        let Some(l) = &self.loaded else { return };
        match surface::add_wall(&l.graph, &l.track, k, side) {
            Ok((edits, id)) => {
                let label = if side == Side::Left { "Add a wall on the left" } else { "Add a wall on the right" };
                self.part_changes(label, "", Vec::new(), edits);
                self.part = Some(id);
            }
            Err(e) => self.edit_error = Some(format!("{e:#}")),
        }
    }

    fn wall_editor(&mut self, ui: &mut egui::Ui, k: usize, id: i32) {
        let Some(l) = &self.loaded else { return };
        let Some(w) = l.track.segments[k].ws.iter().find(|w| w.id == id).cloned() else { return };
        let mut changes: Option<(&str, String, Vec<Change>)> = None;
        let mut moved_to: Option<f64> = None;
        let c = |f: &str, v: f64| Change { id, field: f.into(), value: serde_json::json!(v) };
        ui.label(RichText::new("Wall").strong().size(15.0));
        egui::Grid::new("wall_edit").num_columns(2).spacing([12.0, 5.0]).show(ui, |ui| {
            ui.label("Type");
            let mut ty = w.wall_type;
            egui::ComboBox::from_id_salt("wall_type").selected_text(wall_name(ty)).width(180.0).show_ui(ui, |ui| {
                for (v, name) in surface::WALL_TYPES {
                    ui.selectable_value(&mut ty, *v, *name);
                }
            });
            if ty != w.wall_type {
                changes = Some(("Change wall type", String::new(), vec![Change { id, field: "wall_type".into(), value: serde_json::json!(ty) }]));
            }
            ui.end_row();

            ui.label("Where").on_hover_text("Distance of the wall's middle from the centre line. Type e.g. 12 left or 8.5 right.");
            ui.horizontal(|ui| {
                let (mut a, mut b) = (w.dlat[0], w.dlat[1]);
                ui.label("start");
                let ra = ui.add(side_value(&mut a, 0.05));
                ui.label("end");
                let rb = ui.add(side_value(&mut b, 0.05));
                if (ra.changed() && a != w.dlat[0]) || (rb.changed() && b != w.dlat[1]) {
                    changes = Some(("Move a wall", format!("part{id}.dlat"), vec![c("dlat0", a), c("dlat1", b)]));
                    moved_to = Some(0.5 * (a + b));
                }
            });
            ui.end_row();

            ui.label("Height");
            ui.horizontal(|ui| {
                let (mut a, mut b) = (w.height[0], w.height[1]);
                ui.label("start");
                let ra = ui.add(egui::DragValue::new(&mut a).range(0.0..=100.0).speed(0.02).suffix(" m").max_decimals(2));
                ui.label("end");
                let rb = ui.add(egui::DragValue::new(&mut b).range(0.0..=100.0).speed(0.02).suffix(" m").max_decimals(2));
                if (ra.changed() && a != w.height[0]) || (rb.changed() && b != w.height[1]) {
                    changes = Some(("Change wall height", format!("part{id}.height"), vec![c("height0", a), c("height1", b)]));
                }
            });
            ui.end_row();

            ui.label("Thickness");
            ui.horizontal(|ui| {
                let (mut a, mut b) = (w.width[0], w.width[1]);
                ui.label("start");
                let ra = ui.add(egui::DragValue::new(&mut a).range(0.02..=50.0).speed(0.01).suffix(" m").max_decimals(2));
                ui.label("end");
                let rb = ui.add(egui::DragValue::new(&mut b).range(0.02..=50.0).speed(0.01).suffix(" m").max_decimals(2));
                if (ra.changed() && a != w.width[0]) || (rb.changed() && b != w.width[1]) {
                    if let Some(l) = &self.loaded {
                        changes = Some(("Change wall thickness", format!("part{id}.width"), surface::wall_width(&l.graph, id, a, b)));
                    }
                }
            });
            ui.end_row();
        });
        if let Some((label, key, ch)) = changes {
            // Passing another wall: keep the piece's walls in right-to-left order.
            let resort = moved_to.and_then(|m| self.loaded.as_ref().and_then(|l| surface::resort_walls(&l.graph, &l.track, k, id, m)));
            self.part_changes(label, &key, ch, resort.into_iter().collect());
        }
        self.texture_row(ui, id);
        ui.horizontal(|ui| {
            if ui.button("Delete this wall").clicked() {
                if let Some(l) = &self.loaded {
                    match surface::delete_part(&l.graph, &l.track, k, id) {
                        Ok(edits) => {
                            self.part_changes("Delete a wall", "", Vec::new(), edits);
                            self.part = None;
                        }
                        Err(e) => self.edit_error = Some(format!("{e:#}")),
                    }
                }
            }
        });
        if self.part.is_some() {
            self.copy_row(ui, k, id, true);
        }
    }

    fn strip_editor(&mut self, ui: &mut egui::Ui, k: usize, id: i32) {
        let Some(l) = &self.loaded else { return };
        let s = &l.track.segments[k];
        let Some(f) = s.fs.iter().find(|f| f.id == id).cloned() else { return };
        let limits = [surface::strip_edge_limits(&l.track, k, id, 0), surface::strip_edge_limits(&l.track, k, id, 1)];
        let mut changes: Option<(&str, String, Vec<Change>)> = None;
        ui.label(RichText::new("Ground strip").strong().size(15.0));
        egui::Grid::new("strip_edit").num_columns(2).spacing([12.0, 5.0]).show(ui, |ui| {
            ui.label("Surface");
            let mut v = f.surface;
            egui::ComboBox::from_id_salt("strip_surface").selected_text(surface_name(v)).width(180.0).show_ui(ui, |ui| {
                for (n, name) in surface::SURFACES {
                    ui.selectable_value(&mut v, *n, *name);
                }
            });
            if v != f.surface {
                changes = Some(("Change surface", String::new(), vec![Change { id, field: "i15".into(), value: serde_json::json!(v) }]));
            }
            ui.end_row();

            ui.label("Right edge").on_hover_text("Where this strip starts; it runs left to the next strip. Type e.g. 4 right or 2.5 left.");
            ui.horizontal(|ui| {
                let (mut a, mut b) = (f.dlat[0], f.dlat[1]);
                ui.label("start");
                let ra = ui.add(side_value(&mut a, 0.05).range(limits[0].0..=limits[0].1));
                ui.label("end");
                let rb = ui.add(side_value(&mut b, 0.05).range(limits[1].0..=limits[1].1));
                if (ra.changed() && a != f.dlat[0]) || (rb.changed() && b != f.dlat[1]) {
                    let c = |fl: &str, v: f64| Change { id, field: fl.into(), value: serde_json::json!(v) };
                    changes = Some(("Move a strip edge", format!("part{id}.dlat"), vec![c("dlat0", a), c("dlat1", b)]));
                }
            });
            ui.end_row();
        });
        if let Some((label, key, ch)) = changes {
            self.part_changes(label, &key, ch, Vec::new());
        }
        self.texture_row(ui, id);
        ui.horizontal(|ui| {
            if ui.button("Split in two").on_hover_text("Makes two strips side by side, so one half can be changed.").clicked() {
                if let Some(l) = &self.loaded {
                    match surface::split_strip(&l.graph, &l.track, k, id) {
                        Ok((edits, new_id)) => {
                            self.part_changes("Split a strip", "", Vec::new(), edits);
                            self.part = Some(new_id);
                        }
                        Err(e) => self.edit_error = Some(format!("{e:#}")),
                    }
                }
            }
            if ui.button("Remove").on_hover_text("The strip to its right widens to fill the gap.").clicked() {
                if let Some(l) = &self.loaded {
                    match surface::delete_part(&l.graph, &l.track, k, id) {
                        Ok(edits) => {
                            self.part_changes("Remove a strip", "", Vec::new(), edits);
                            self.part = None;
                        }
                        Err(e) => self.edit_error = Some(format!("{e:#}")),
                    }
                }
            }
        });
        if self.part.is_some() {
            self.copy_row(ui, k, id, false);
        }
    }

    /// The texture of a wall's or strip's faces: pick one the track already uses, or a .mip file.
    fn texture_row(&mut self, ui: &mut egui::Ui, id: i32) {
        let Some(l) = &self.loaded else { return };
        let faces = surface::textured_faces(&l.graph, id);
        if faces.is_empty() {
            return;
        }
        let apps: Vec<i32> = faces.iter().map(|f| f.1).collect();
        let mut names: Vec<String> = faces.iter().filter_map(|(_, a)| surface::texture_of(&l.graph, *a).map(|t| t.1)).collect();
        names.dedup();
        let now = if names.len() == 1 { names[0].clone() } else { format!("{} different", names.len()) };
        let textures = surface::track_textures(&l.graph);
        let mut chosen: Option<i32> = None;
        let mut other = false;
        ui.horizontal(|ui| {
            ui.label("Texture").on_hover_text(if faces.len() > 1 {
                "Sets the texture on every side of the wall"
            } else {
                "The picture drawn on this strip"
            });
            egui::ComboBox::from_id_salt(("part_texture", id)).selected_text(&now).width(220.0).height(360.0).show_ui(ui, |ui| {
                for (tid, file) in &textures {
                    if ui.selectable_label(names.len() == 1 && names[0].eq_ignore_ascii_case(file), file).clicked() {
                        chosen = Some(*tid);
                    }
                }
                ui.separator();
                if ui.selectable_label(false, "Another .mip file…").clicked() {
                    other = true;
                }
            });
        });
        if let Some(t) = chosen {
            self.part_changes("Change texture", "", surface::set_texture(&apps, t), Vec::new());
        }
        if other {
            self.pick_texture_file(&apps);
        }
    }

    fn pick_texture_file(&mut self, apps: &[i32]) {
        let Some(l) = &self.loaded else { return };
        let Some(folder) = l.folder.clone() else {
            self.edit_error = Some("Save the track first, so there's a folder for the texture.".into());
            return;
        };
        let start = if folder.join("trackmat").is_dir() { folder.join("trackmat") } else { folder.clone() };
        let Some(file) = rfd::FileDialog::new().set_title("Pick a texture (.mip)").add_filter("Texture", &["mip"]).set_directory(start).pick_file() else {
            return;
        };
        match surface::bring_in_texture(&file, &folder) {
            Ok((name, note)) => {
                let Some(l) = &self.loaded else { return };
                let (node, tex) = surface::texture_for_file(&l.graph, &name);
                self.part_changes("Change texture", "", surface::set_texture(apps, tex), node.into_iter().collect());
                self.part_note = note.map(|n| format!("Texture: {n}."));
            }
            Err(e) => self.edit_error = Some(format!("{e:#}")),
        }
    }

    /// "Copy to pieces A to B" for the picked wall or strip.
    fn copy_row(&mut self, ui: &mut egui::Ui, k: usize, id: i32, wall: bool) {
        let n = self.loaded.as_ref().map_or(1, |l| l.track.segments.len());
        let mut go = false;
        ui.horizontal(|ui| {
            ui.label("Copy to pieces");
            ui.add(egui::DragValue::new(&mut self.copy_to.0).range(1..=n).speed(0.1));
            ui.label("to");
            ui.add(egui::DragValue::new(&mut self.copy_to.1).range(1..=n).speed(0.1));
            go = ui
                .button("Copy")
                .on_hover_text(if wall {
                    "The wall on the same side of each of those pieces gets this wall's type, height, thickness and texture."
                } else {
                    "The strip in the same place on each of those pieces gets this strip's surface and texture."
                })
                .clicked();
        });
        if !go {
            return;
        }
        let (a, b) = (self.copy_to.0.min(self.copy_to.1) - 1, self.copy_to.0.max(self.copy_to.1) - 1);
        let Some(l) = &self.loaded else { return };
        if wall {
            let (changes, missing) = surface::copy_wall_to(&l.graph, &l.track, id, a..=b);
            self.part_changes("Copy a wall along the track", "", changes, Vec::new());
            self.part_note = (missing > 0).then(|| {
                format!("{missing} of those pieces have no wall on that side; use \"Add a wall\" there first.")
            });
        } else {
            let changes = surface::copy_strip_to(&l.graph, &l.track, id, a..=b);
            self.part_changes("Copy a strip along the track", "", changes, Vec::new());
        }
        let _ = k;
    }
}
