//! Track tab, Objects mode: pick placed objects on the map (click, Ctrl+click, drag a box),
//! move them by dragging, turn them with the round handle, and the side panel to add, copy,
//! delete, lock and place them by number.

use super::*;
use ttk::track::edit::Change;
use ttk::track::objects::{self, NewObject};

/// A drag on the map in progress.
pub(super) enum Drag {
    /// Moving the selection; `last` is the world point under the mouse last frame.
    Move { key: String, last: [f64; 2] },
    /// Turning the selection about (cx, cy); `last` is the mouse's angle last frame.
    Turn { key: String, cx: f64, cy: f64, last: f64 },
    /// Drawing a box to select what's inside.
    Box { from: egui::Pos2 },
}

/// How far (in screen pixels) the turn handle sits from the middle of the selection.
const HANDLE_PX: f32 = 48.0;

const DOT: Color32 = Color32::from_rgb(255, 200, 40);
const DOT_SELECTED: Color32 = Color32::from_rgb(255, 110, 25);
const DOT_LOCKED: Color32 = Color32::from_rgb(140, 140, 148);

impl TrackView {
    /// Object marker triangles for the 3D/map layer, coloured by selected/locked.
    pub(super) fn object_verts(&self) -> Vec<f32> {
        let Some(l) = &self.loaded else { return Vec::new() };
        if !self.show_objects {
            return Vec::new();
        }
        let objs = &l.track.objects;
        mesh::objects(&l.track, l.mesh.origin, |k| {
            let id = objs[k].id;
            if self.obj_sel.contains(&id) {
                mesh::OBJECT_SELECTED
            } else if self.locked.contains(&id) {
                mesh::OBJECT_LOCKED
            } else {
                mesh::OBJECT_COLOR
            }
        })
    }

    /// Selected objects that aren't locked (the ones a drag moves).
    fn movable(&self) -> Vec<i32> {
        self.obj_sel.iter().copied().filter(|id| !self.locked.contains(id)).collect()
    }

    fn map_mpp(&self, rect: egui::Rect) -> f64 {
        2.0 * self.cam.half_h / rect.height().max(1.0) as f64
    }

    fn to_world(&self, rect: egui::Rect, p: egui::Pos2, origin: [f64; 3]) -> [f64; 2] {
        let mpp = self.map_mpp(rect);
        [
            self.cam.cx + (p.x - rect.center().x) as f64 * mpp + origin[0],
            self.cam.cy - (p.y - rect.center().y) as f64 * mpp + origin[1],
        ]
    }

    fn to_screen(&self, rect: egui::Rect, x: f64, y: f64, origin: [f64; 3]) -> egui::Pos2 {
        let mpp = self.map_mpp(rect);
        egui::pos2(
            rect.center().x + ((x - origin[0] - self.cam.cx) / mpp) as f32,
            rect.center().y - ((y - origin[1] - self.cam.cy) / mpp) as f32,
        )
    }

    /// Unlocked objects under screen point `p`, nearest first.
    fn object_hits(&self, rect: egui::Rect, p: egui::Pos2) -> Vec<i32> {
        let Some(l) = &self.loaded else { return Vec::new() };
        let reach = ((1.5 / self.map_mpp(rect)) as f32).max(9.0);
        let mut hits: Vec<(f32, i32)> = l
            .track
            .objects
            .iter()
            .filter(|o| !self.locked.contains(&o.id))
            .map(|o| (self.to_screen(rect, o.x, o.y, l.mesh.origin).distance(p), o.id))
            .filter(|(d, _)| *d <= reach)
            .collect();
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        hits.into_iter().map(|h| h.1).collect()
    }

    /// The turn handle: (middle of the selection in the world, that on screen, the handle).
    fn turn_handle(&self, rect: egui::Rect) -> Option<([f64; 2], egui::Pos2, egui::Pos2)> {
        let l = self.loaded.as_ref()?;
        let ids = self.movable();
        let objs: Vec<&model::Placed> = l.track.objects.iter().filter(|o| ids.contains(&o.id)).collect();
        let first = objs.iter().find(|o| o.id == ids[0]).or(objs.first())?;
        let n = objs.len() as f64;
        let c = [objs.iter().map(|o| o.x).sum::<f64>() / n, objs.iter().map(|o| o.y).sum::<f64>() / n];
        let sc = self.to_screen(rect, c[0], c[1], l.mesh.origin);
        let (s, co) = first.heading.sin_cos();
        let h = sc + egui::vec2(co as f32, -s as f32) * HANDLE_PX;
        Some((c, sc, h))
    }

    fn select(&mut self, ids: Vec<i32>) {
        self.obj_sel = ids;
        self.objects_dirty = true;
        self.obj_scroll = true;
    }

    /// Turn object number changes into an undoable step.
    fn object_changes(&mut self, label: &str, key: &str, changes: Vec<Change>) {
        let Some(l) = &self.loaded else { return };
        let steps = changes
            .into_iter()
            .map(|c| {
                let old = l.graph.node(c.id).and_then(|n| n.get(&c.field)).cloned().unwrap_or(serde_json::Value::Null);
                (c.id, c.field, old, c.value)
            })
            .collect();
        self.edit(saving::EditStep::objects(label, key, steps, Vec::new()));
    }

    /// Clicks and drags on the map in Objects mode.
    pub(super) fn objects_map_input(&mut self, ui: &egui::Ui, rect: egui::Rect, resp: &egui::Response) {
        let Some(origin) = self.loaded.as_ref().map(|l| l.mesh.origin) else { return };
        if !self.show_objects {
            return;
        }
        let ctrl = ui.input(|i| i.modifiers.command);
        let primary = egui::PointerButton::Primary;

        if resp.drag_started_by(primary) {
            let p = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).unwrap_or(rect.center());
            let w = self.to_world(rect, p, origin);
            let on_handle = self.turn_handle(rect).filter(|(_, _, h)| h.distance(p) <= 11.0);
            self.obj_key += 1;
            if let Some((c, _, _)) = on_handle {
                let a = (w[1] - c[1]).atan2(w[0] - c[0]);
                self.obj_drag = Some(Drag::Turn { key: format!("objturn{}", self.obj_key), cx: c[0], cy: c[1], last: a });
            } else {
                let hits = self.object_hits(rect, p);
                if hits.iter().any(|h| self.obj_sel.contains(h)) {
                    self.obj_drag = Some(Drag::Move { key: format!("objmove{}", self.obj_key), last: w });
                } else if let Some(&h) = hits.first() {
                    let mut sel = if ctrl { self.obj_sel.clone() } else { Vec::new() };
                    sel.push(h);
                    self.select(sel);
                    self.obj_drag = Some(Drag::Move { key: format!("objmove{}", self.obj_key), last: w });
                } else {
                    self.obj_drag = Some(Drag::Box { from: p });
                }
            }
        }

        if resp.dragged_by(primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                let w = self.to_world(rect, p, origin);
                // (label, key, moved by, turned by about)
                let act = match &mut self.obj_drag {
                    Some(Drag::Move { key, last }) => {
                        let d = [w[0] - last[0], w[1] - last[1]];
                        *last = w;
                        (d != [0.0, 0.0]).then(|| (key.clone(), Some(d), None))
                    }
                    Some(Drag::Turn { key, cx, cy, last }) => {
                        let a = (w[1] - *cy).atan2(w[0] - *cx);
                        let da = (a - *last + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
                        *last = a;
                        (da != 0.0).then(|| (key.clone(), None, Some((*cx, *cy, da))))
                    }
                    _ => None,
                };
                if let Some((key, mv, turn)) = act {
                    let ids = self.movable();
                    if let Some(l) = &self.loaded {
                        if let Some(d) = mv {
                            let c = objects::move_by(&l.graph, &ids, d[0], d[1], 0.0);
                            self.object_changes("Move objects", &key, c);
                        } else if let Some((cx, cy, da)) = turn {
                            let c = objects::turn_by(&l.graph, &ids, cx, cy, da);
                            self.object_changes("Turn objects", &key, c);
                        }
                    }
                }
            }
        }

        if resp.drag_stopped() {
            if let Some(Drag::Box { from }) = self.obj_drag.take() {
                let to = ui.input(|i| i.pointer.latest_pos()).unwrap_or(from);
                let r = egui::Rect::from_two_pos(from, to);
                if let Some(l) = &self.loaded {
                    let inside: Vec<i32> = l
                        .track
                        .objects
                        .iter()
                        .filter(|o| !self.locked.contains(&o.id) && r.contains(self.to_screen(rect, o.x, o.y, origin)))
                        .map(|o| o.id)
                        .collect();
                    let mut sel = if ctrl { self.obj_sel.clone() } else { Vec::new() };
                    for id in inside {
                        if !sel.contains(&id) {
                            sel.push(id);
                        }
                    }
                    self.select(sel);
                }
            }
            self.obj_drag = None;
        }

        if resp.clicked_by(primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                let hits = self.object_hits(rect, p);
                if hits.is_empty() {
                    if !ctrl {
                        self.select(Vec::new());
                    }
                    self.obj_cycle = None;
                } else if ctrl {
                    let mut sel = self.obj_sel.clone();
                    match sel.iter().position(|&s| s == hits[0]) {
                        Some(k) => {
                            sel.remove(k);
                        }
                        None => sel.push(hits[0]),
                    }
                    self.select(sel);
                } else {
                    // Clicking the same spot again steps to the object under it.
                    let k = match &self.obj_cycle {
                        Some((at, list, k)) if at.distance(p) < 5.0 && *list == hits => (k + 1) % hits.len(),
                        _ => 0,
                    };
                    self.select(vec![hits[k]]);
                    self.obj_cycle = Some((p, hits, k));
                }
            }
        }
    }

    /// Dots for the objects on the map, the selection, the box being drawn and the turn handle.
    pub(super) fn objects_overlay(&self, painter: &egui::Painter, rect: egui::Rect) {
        let Some(l) = &self.loaded else { return };
        let origin = l.mesh.origin;
        let area = rect.expand(10.0);
        let objects_mode = self.mode == Mode::Objects;
        for o in &l.track.objects {
            let q = self.to_screen(rect, o.x, o.y, origin);
            if !area.contains(q) {
                continue;
            }
            let selected = self.obj_sel.contains(&o.id);
            let locked = self.locked.contains(&o.id);
            if selected {
                let (s, c) = o.heading.sin_cos();
                painter.line_segment([q, q + egui::vec2(c as f32, -s as f32) * 12.0], egui::Stroke::new(2.0f32, DOT_SELECTED));
                painter.circle(q, 5.0, DOT_SELECTED, egui::Stroke::new(1.5f32, Color32::WHITE));
            } else if objects_mode {
                painter.circle_filled(q, 3.0, if locked { DOT_LOCKED } else { DOT });
            }
        }
        if !objects_mode {
            return;
        }
        if let Some(Drag::Box { from }) = &self.obj_drag {
            if let Some(to) = painter.ctx().input(|i| i.pointer.latest_pos()) {
                let r = egui::Rect::from_two_pos(*from, to);
                painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 170, 60, 30));
                painter.rect_stroke(r, 0.0, egui::Stroke::new(1.0f32, DOT_SELECTED), egui::StrokeKind::Inside);
            }
        }
        if let Some((_, c, h)) = self.turn_handle(rect) {
            painter.line_segment([c, h], egui::Stroke::new(1.5f32, Color32::WHITE));
            painter.circle(h, 7.0, Color32::from_rgb(70, 140, 255), egui::Stroke::new(1.5f32, Color32::WHITE));
        }
    }

    /// Delete, Ctrl+D and Escape in Objects mode (not while typing in a box).
    pub(super) fn objects_keys(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        use egui::{Key, KeyboardShortcut, Modifiers};
        let (del, dup, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Delete),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::D)),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if del {
            self.delete_selected();
        }
        if dup {
            self.duplicate_selected();
        }
        if esc {
            self.select(Vec::new());
        }
    }

    fn delete_selected(&mut self) {
        let ids = self.movable();
        let skipped = self.obj_sel.len() - ids.len();
        if ids.is_empty() {
            if skipped > 0 {
                self.obj_note = Some("Locked objects can't be deleted. Unlock them first.".into());
            }
            return;
        }
        let Some(l) = &self.loaded else { return };
        match objects::delete(&l.graph, &ids) {
            Ok(edits) => {
                let label = if ids.len() == 1 { "Delete an object".to_string() } else { format!("Delete {} objects", ids.len()) };
                self.edit(saving::EditStep::objects(&label, "", Vec::new(), edits));
                self.obj_note = Some(if skipped > 0 {
                    format!("Deleted {}. {skipped} locked object(s) were left alone.", ids.len())
                } else {
                    format!("Deleted {}. Undo (Ctrl+Z) brings them back.", ids.len())
                });
                self.select(self.obj_sel.iter().copied().filter(|id| !ids.contains(id)).collect());
            }
            Err(e) => self.obj_note = Some(format!("Couldn't delete: {e:#}")),
        }
    }

    fn duplicate_selected(&mut self) {
        let ids = self.obj_sel.clone();
        if ids.is_empty() {
            return;
        }
        let Some(l) = &self.loaded else { return };
        match objects::duplicate(&l.graph, &ids, 5.0, -5.0) {
            Ok((edits, new_ids)) => {
                self.edit(saving::EditStep::objects("Copy objects", "", Vec::new(), edits));
                self.obj_note = Some(if new_ids.len() == 1 {
                    "Made a copy, 5 m along. Drag it into place.".to_string()
                } else {
                    format!("Made {} copies, 5 m along. Drag them into place.", new_ids.len())
                });
                self.select(new_ids);
            }
            Err(e) => self.obj_note = Some(format!("Couldn't copy: {e:#}")),
        }
    }

    fn toggle_lock(&mut self) {
        if self.obj_sel.is_empty() {
            return;
        }
        let all_locked = self.obj_sel.iter().all(|id| self.locked.contains(id));
        for id in &self.obj_sel {
            if all_locked {
                self.locked.remove(id);
            } else {
                self.locked.insert(*id);
            }
        }
        self.objects_dirty = true;
        // With no unsaved changes the positions on disk match, so the locks can be written now;
        // otherwise they're written with the next save.
        if !self.dirty() {
            self.save_locks();
        }
    }

    /// Write the locks next to the track file.
    pub(super) fn save_locks(&mut self) {
        let Some(l) = &self.loaded else { return };
        let Some(p) = &l.save_path else { return };
        if let Err(e) = objects::save_locks(p, &l.track, &self.locked) {
            self.obj_note = Some(format!("Couldn't remember the locks: {e:#}"));
        }
    }

    fn view_centre(&self) -> [f64; 2] {
        let o = self.loaded.as_ref().map_or([0.0; 3], |l| l.mesh.origin);
        match self.view {
            View::Map => [self.cam.cx + o[0], self.cam.cy + o[1]],
            View::ThreeD => [self.cam.target[0] + o[0], self.cam.target[1] + o[1]],
        }
    }

    fn add_objects_dialog(&mut self) {
        let Some(l) = &self.loaded else { return };
        let Some(folder) = l.folder.clone() else {
            self.obj_note = Some("Save the track first, so there's a folder to put the objects in.".into());
            return;
        };
        let Some(files) = rfd::FileDialog::new()
            .set_title("Pick objects (.3do) to add")
            .add_filter("Object", &["3do"])
            .set_directory(&folder)
            .pick_files()
        else {
            return;
        };
        let shared = shared_folder(&folder);
        let mut names = Vec::new();
        let (mut copied, mut missing, mut kept) = (Vec::new(), Vec::new(), Vec::new());
        for f in &files {
            match objects::bring_in(f, &folder, shared.as_deref()) {
                Ok(b) => {
                    names.push(b.file);
                    copied.extend(b.copied);
                    missing.extend(b.not_found);
                    kept.extend(b.kept_existing);
                }
                Err(e) => {
                    self.obj_note = Some(format!("Couldn't add {}: {e:#}", f.display()));
                    return;
                }
            }
        }
        // In a row across the middle of the view, sitting on the ground.
        let c = self.view_centre();
        let spacing = 8.0;
        let start = c[0] - spacing * (names.len() as f64 - 1.0) / 2.0;
        let Some(l) = &self.loaded else { return };
        let new: Vec<NewObject> = names
            .iter()
            .enumerate()
            .map(|(k, f)| {
                let (x, y) = (start + spacing * k as f64, c[1]);
                NewObject { file: f.clone(), x, y, z: l.track.ground_at(x, y), heading: 0.0 }
            })
            .collect();
        match objects::add(&l.graph, &new) {
            Ok((edits, ids)) => {
                let label = if ids.len() == 1 { "Add an object".to_string() } else { format!("Add {} objects", ids.len()) };
                self.edit(saving::EditStep::objects(&label, "", Vec::new(), edits));
                self.select(ids.clone());
                let mut note = format!(
                    "Added {} in the middle of the view. Drag {} into place.",
                    if ids.len() == 1 { "the object".to_string() } else { format!("{} objects", ids.len()) },
                    if ids.len() == 1 { "it" } else { "them" }
                );
                if !copied.is_empty() {
                    note += &format!(" Copied {} into the track folder.", copied.join(", "));
                }
                if !kept.is_empty() {
                    note += &format!(" The track folder already had a different {}; that one is used.", kept.join(", "));
                }
                if !missing.is_empty() {
                    note += &format!(
                        " Couldn't find {} (used by the object); put {} in the track folder or the game won't load the track.",
                        missing.join(", "),
                        if missing.len() == 1 { "it" } else { "them" }
                    );
                }
                self.obj_note = Some(note);
            }
            Err(e) => self.obj_note = Some(format!("Couldn't add: {e:#}")),
        }
    }

    /// The side panel in Objects mode.
    pub(super) fn objects_panel(&mut self, ui: &mut egui::Ui) {
        let Some(l) = &self.loaded else { return };
        let n_sel = self.obj_sel.len();
        let all_locked = n_sel > 0 && self.obj_sel.iter().all(|id| self.locked.contains(id));
        let has_folder = l.folder.is_some();
        ui.horizontal_wrapped(|ui| {
            let big = |t: &str| egui::Button::new(RichText::new(t).size(15.0));
            if ui.add_enabled(has_folder, big("Add objects…")).on_hover_text("Pick .3do files. They're copied into the track folder if they aren't there yet.").clicked() {
                self.add_objects_dialog();
            }
            if ui.add_enabled(n_sel > 0, big("Make a copy")).on_hover_text("Ctrl+D").clicked() {
                self.duplicate_selected();
            }
            if ui.add_enabled(n_sel > 0, big("Delete")).on_hover_text("Delete key").clicked() {
                self.delete_selected();
            }
            if ui.add_enabled(n_sel > 0, big(if all_locked { "Unlock" } else { "Lock" }))
                .on_hover_text("Locked objects can't be picked on the map, so they can't be moved by accident.")
                .clicked()
            {
                self.toggle_lock();
            }
            if ui.add_enabled(n_sel > 0, big("Put on the ground")).on_hover_text("Sets the height to the ground under each object.").clicked() {
                if let Some(l) = &self.loaded {
                    let c = objects::snap_to_ground(&l.graph, &l.track, &self.movable());
                    self.object_changes("Put objects on the ground", "", c);
                }
            }
        });
        if let Some(n) = &self.obj_note {
            ui.label(RichText::new(n).color(Color32::from_rgb(200, 210, 230)));
        }

        // The selection, by number
        ui.add_space(4.0);
        let Some(l) = &self.loaded else { return };
        if n_sel == 0 {
            ui.label(RichText::new("Click an object on the map to select it. Ctrl+click adds more; drag a box around several.").weak());
        } else if n_sel == 1 {
            let id = self.obj_sel[0];
            if let Some(o) = l.track.objects.iter().find(|o| o.id == id).cloned() {
                let locked = self.locked.contains(&id);
                ui.label(RichText::new(format!("{}{}", o.file, if locked { "  (locked)" } else { "" })).strong().size(15.0));
                let mut change: Option<(&str, &str, serde_json::Value)> = None;
                egui::Grid::new("obj_edit").num_columns(2).spacing([12.0, 5.0]).show(ui, |ui| {
                    for (label, field, v) in [("Across (x)", "x", o.x), ("Up the map (y)", "y", o.y), ("Height", "z", o.z)] {
                        ui.label(label);
                        let mut n = v;
                        let r = ui.add_enabled(!locked, egui::DragValue::new(&mut n).speed(0.1).suffix(" m").max_decimals(3));
                        if r.changed() && n != v {
                            change = Some(("Move an object", field, serde_json::json!(n)));
                        }
                        ui.end_row();
                    }
                    ui.label("Direction");
                    let deg = ttk::track::edit::wrap_heading(o.heading).to_degrees();
                    let mut n = deg;
                    let r = ui.add_enabled(!locked, egui::DragValue::new(&mut n).speed(0.5).suffix("°").max_decimals(2))
                        .on_hover_text("0° faces right on the map (east); it turns anticlockwise.");
                    if r.changed() && n != deg {
                        let rad = ttk::track::edit::wrap_heading(n.to_radians());
                        change = Some(("Turn an object", "rot1", serde_json::json!(rad)));
                    }
                    ui.end_row();
                });
                if let Some((label, field, v)) = change {
                    let key = format!("obj{id}.{field}");
                    self.object_changes(label, &key, vec![Change { id, field: field.into(), value: v }]);
                }
                if locked {
                    ui.label(RichText::new("Locked: unlock it to move it.").weak());
                }
            }
        } else {
            ui.label(RichText::new(format!("{n_sel} objects selected")).strong().size(15.0));
            ui.label(RichText::new("Drag them on the map to move them; drag the round blue handle to turn them.").weak());
        }

        // Every object, searchable
        ui.separator();
        let Some(l) = &self.loaded else { return };
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Objects ({})", l.track.objects.len())).strong());
            ui.add(egui::TextEdit::singleline(&mut self.obj_filter).hint_text("Find by name").desired_width(170.0));
        });
        ui.label(RichText::new(format!(" {:<22}{:>7}{:>7}", "File", "x", "y")).monospace().weak());
        let filter = self.obj_filter.to_ascii_lowercase();
        let rows: Vec<(i32, String, f64, f64)> = l
            .track
            .objects
            .iter()
            .filter(|o| filter.is_empty() || o.file.to_ascii_lowercase().contains(&filter))
            .map(|o| (o.id, o.file.clone(), o.x, o.y))
            .collect();
        // Every row is exactly this tall, so the list can jump straight to the selection.
        let row_h = (ui.text_style_height(&egui::TextStyle::Monospace) + 2.0 * ui.spacing().button_padding.y)
            .max(ui.spacing().interact_size.y);
        let row_step = row_h + ui.spacing().item_spacing.y;
        let mut area = egui::ScrollArea::vertical().id_salt("obj_list").auto_shrink([false, false]);
        if self.obj_scroll {
            self.obj_scroll = false;
            if let Some(k) = self.obj_sel.first().and_then(|id| rows.iter().position(|r| r.0 == *id)) {
                area = area.vertical_scroll_offset((k as f32 * row_step - 80.0).max(0.0));
            }
        }
        let mut clicked: Option<(i32, f64, f64)> = None;
        let ctrl = ui.input(|i| i.modifiers.command);
        area.show_rows(ui, row_h, rows.len(), |ui, range| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            for (id, file, x, y) in &rows[range] {
                let lock = if self.locked.contains(id) { " lock" } else { "" };
                let text = format!("{file:<22}{x:>7.0}{y:>7.0}{lock}");
                let mut rt = RichText::new(text).monospace();
                if self.locked.contains(id) {
                    rt = rt.color(DOT_LOCKED);
                }
                let w = ui.available_width();
                if ui.add_sized([w, row_h], egui::SelectableLabel::new(self.obj_sel.contains(id), rt)).clicked() {
                    clicked = Some((*id, *x, *y));
                }
            }
        });
        if let Some((id, x, y)) = clicked {
            let mut sel = if ctrl { self.obj_sel.clone() } else { Vec::new() };
            match sel.iter().position(|&s| s == id) {
                Some(k) if ctrl => {
                    sel.remove(k);
                }
                Some(_) => {}
                None => sel.push(id),
            }
            self.obj_sel = sel;
            self.objects_dirty = true;
            // show it in the middle of the view
            if let Some(l) = &self.loaded {
                let o = l.mesh.origin;
                self.cam.cx = x - o[0];
                self.cam.cy = y - o[1];
                self.cam.target = [x - o[0], y - o[1], l.track.ground_at(x, y) - o[2]];
            }
        }
    }
}
