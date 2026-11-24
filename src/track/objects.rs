//! Placed objects (scenery .3do files): add, remove, copy, move and turn them.
//!
//! In the .ptf every placed object is a `TSOReferenceDescriptor` (position, rotation, and a
//! reference to a `TSODescriptor` naming the .3do file). Placements of the same file share one
//! TSO. The track's root lists the placements as `tsoref_count`, `tsoref_size` (4 bytes per
//! entry) and `tsoref[0..n]`.
//!
//! Adding or removing objects changes whole objects (nodes), so those edits come back as
//! [`NodeEdit`]s (before/after), which also make Undo simple. Moving and turning only change
//! numbers, so they come back as [`Change`]s like the shape edits.

use super::edit::{wrap_heading, Change};
use super::graph::{Graph, Node};
use super::model::Track;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One object replaced, added (`before` None) or removed (`after` None).
#[derive(Clone, Debug)]
pub struct NodeEdit {
    pub id: i32,
    pub before: Option<Node>,
    pub after: Option<Node>,
}

/// An object to place.
#[derive(Clone, Debug)]
pub struct NewObject {
    /// The .3do file name, as the track refers to it (e.g. `building07a.3do`).
    pub file: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub heading: f64,
}

fn field(name: &str, kind: &str, v: Value) -> (String, String, Value) {
    (name.to_string(), kind.to_string(), v)
}

/// A new TSO naming a .3do file, laid out like the stock files.
fn new_tso(id: i32, file: &str) -> Node {
    Node {
        id,
        class: "TSODescriptor".into(),
        fields: vec![
            field("base", "i32", json!(1)),
            field("name", "str", Value::Null),
            field("version", "i32", json!(3)),
            field("file", "str", json!(file)),
            field("s10", "str", json!(format!("(an instance of '{file}')"))),
            field("flag", "u8", json!(0)),
        ],
    }
}

/// A new placement of TSO `tso`, laid out like most stock placements (no extras, no sound).
fn new_tsoref(id: i32, tso: i32, o: &NewObject) -> Node {
    Node {
        id,
        class: "TSOReferenceDescriptor".into(),
        fields: vec![
            field("base", "i32", json!(1)),
            field("name", "str", Value::Null),
            field("version", "i32", json!(6)),
            field("x", "f64", json!(o.x)),
            field("y", "f64", json!(o.y)),
            field("z", "f64", json!(o.z)),
            field("rot1", "f64", json!(wrap_heading(o.heading))),
            field("rot2", "f64", json!(0.0)),
            field("rot3", "f64", json!(0.0)),
            field("tso", "obj", json!(tso)),
            field("i40", "i32", json!(0)),
            field("no_extra", "u8", json!(1)),
            field("item_count", "i32", json!(0)),
            field("sound", "str", json!("")),
        ],
    }
}

/// The placements in the root's list, in order.
pub fn tsoref_ids(g: &Graph) -> Vec<i32> {
    g.root().list_ids("tsoref")
}

fn root_edit(g: &Graph, ids: &[i32]) -> Result<NodeEdit> {
    Ok(NodeEdit { id: g.root, before: Some(g.root().clone()), after: Some(g.root().with_list("tsoref", ids)?) })
}

/// The TSO already naming `file` (any case), if there is one.
fn find_tso(g: &Graph, file: &str) -> Option<i32> {
    let mut ids: Vec<i32> = g
        .nodes
        .values()
        .filter(|n| n.class == "TSODescriptor" && n.s("file").is_some_and(|f| f.eq_ignore_ascii_case(file)))
        .map(|n| n.id)
        .collect();
    ids.sort_unstable();
    ids.first().copied()
}

/// Place new objects (at the end of the list). Returns the edits and the new placements' ids.
pub fn add(g: &Graph, objs: &[NewObject]) -> Result<(Vec<NodeEdit>, Vec<i32>)> {
    let mut next = g.next_id();
    let mut edits = Vec::new();
    let mut made_tso: Vec<(String, i32)> = Vec::new();
    let mut ids = tsoref_ids(g);
    let mut new_ids = Vec::new();
    for o in objs {
        let tso = match find_tso(g, &o.file).or_else(|| {
            made_tso.iter().find(|(f, _)| f.eq_ignore_ascii_case(&o.file)).map(|t| t.1)
        }) {
            Some(t) => t,
            None => {
                let id = next;
                next += 1;
                edits.push(NodeEdit { id, before: None, after: Some(new_tso(id, &o.file)) });
                made_tso.push((o.file.clone(), id));
                id
            }
        };
        let id = next;
        next += 1;
        edits.push(NodeEdit { id, before: None, after: Some(new_tsoref(id, tso, o)) });
        ids.push(id);
        new_ids.push(id);
    }
    edits.push(root_edit(g, &ids)?);
    Ok((edits, new_ids))
}

/// Copies of placements `which`, moved by (dx, dy), added at the end of the list.
pub fn duplicate(g: &Graph, which: &[i32], dx: f64, dy: f64) -> Result<(Vec<NodeEdit>, Vec<i32>)> {
    let mut next = g.next_id();
    let mut edits = Vec::new();
    let mut ids = tsoref_ids(g);
    let mut new_ids = Vec::new();
    for &w in which {
        let Some(src) = g.node(w).filter(|n| n.class == "TSOReferenceDescriptor") else { continue };
        let mut n = src.clone();
        n.id = next;
        next += 1;
        n.set("x", json!(src.f("x") + dx))?;
        n.set("y", json!(src.f("y") + dy))?;
        ids.push(n.id);
        new_ids.push(n.id);
        edits.push(NodeEdit { id: n.id, before: None, after: Some(n) });
    }
    edits.push(root_edit(g, &ids)?);
    Ok((edits, new_ids))
}

/// Remove placements `which`, and any TSO nothing uses any more.
pub fn delete(g: &Graph, which: &[i32]) -> Result<Vec<NodeEdit>> {
    let gone: HashSet<i32> = which.iter().copied().collect();
    let ids: Vec<i32> = tsoref_ids(g).into_iter().filter(|id| !gone.contains(id)).collect();
    let mut edits = vec![root_edit(g, &ids)?];
    let mut tsos: HashSet<i32> = HashSet::new();
    for &w in &gone {
        if let Some(n) = g.node(w).filter(|n| n.class == "TSOReferenceDescriptor") {
            if let Some(t) = n.r("tso") {
                tsos.insert(t);
            }
            edits.push(NodeEdit { id: w, before: Some(n.clone()), after: None });
        }
    }
    // A TSO stays if anything left still refers to it.
    for n in g.nodes.values() {
        if gone.contains(&n.id) || n.id == g.root {
            continue;
        }
        for f in &n.fields {
            if f.1 == "obj" {
                if let Some(v) = f.2.as_i64() {
                    tsos.remove(&(v as i32));
                }
            }
        }
    }
    let mut tsos: Vec<i32> = tsos.into_iter().collect();
    tsos.sort_unstable();
    for t in tsos {
        if let Some(n) = g.node(t) {
            edits.push(NodeEdit { id: t, before: Some(n.clone()), after: None });
        }
    }
    Ok(edits)
}

/// Apply node edits forward (`after`) or backward (`before`, in reverse order).
pub fn apply_nodes(g: &mut Graph, edits: &[NodeEdit], forward: bool) {
    let mut apply = |e: &NodeEdit| {
        let n = if forward { &e.after } else { &e.before };
        match n {
            Some(n) => {
                g.nodes.insert(e.id, n.clone());
            }
            None => {
                g.nodes.remove(&e.id);
            }
        }
    };
    if forward {
        edits.iter().for_each(&mut apply);
    } else {
        edits.iter().rev().for_each(&mut apply);
    }
}

/// Move placements by (dx, dy, dz).
pub fn move_by(g: &Graph, which: &[i32], dx: f64, dy: f64, dz: f64) -> Vec<Change> {
    let mut out = Vec::new();
    for &w in which {
        let Some(n) = g.node(w) else { continue };
        for (f, d) in [("x", dx), ("y", dy), ("z", dz)] {
            if d != 0.0 {
                out.push(Change { id: w, field: f.into(), value: json!(n.f(f) + d) });
            }
        }
    }
    out
}

/// Turn placements by `angle` (radians, positive = anticlockwise seen from above) about
/// the point (cx, cy).
pub fn turn_by(g: &Graph, which: &[i32], cx: f64, cy: f64, angle: f64) -> Vec<Change> {
    let (s, c) = angle.sin_cos();
    let mut out = Vec::new();
    for &w in which {
        let Some(n) = g.node(w) else { continue };
        let (x, y) = (n.f("x") - cx, n.f("y") - cy);
        out.push(Change { id: w, field: "x".into(), value: json!(cx + x * c - y * s) });
        out.push(Change { id: w, field: "y".into(), value: json!(cy + x * s + y * c) });
        out.push(Change { id: w, field: "rot1".into(), value: json!(wrap_heading(n.f("rot1") + angle)) });
    }
    out
}

/// Put placements on the ground under them.
pub fn snap_to_ground(g: &Graph, t: &Track, which: &[i32]) -> Vec<Change> {
    which
        .iter()
        .filter_map(|&w| g.node(w).map(|n| (w, n)))
        .map(|(w, n)| Change { id: w, field: "z".into(), value: json!(t.ground_at(n.f("x"), n.f("y"))) })
        .collect()
}

// ------------------------------------------------------------------ object files

/// Names of other files (textures, other models) a .3do file mentions.
pub fn files_used_by(model: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let ok = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.';
    let mut k = 0;
    while k < model.len() {
        if !ok(model[k]) {
            k += 1;
            continue;
        }
        let start = k;
        while k < model.len() && ok(model[k]) {
            k += 1;
        }
        let word = String::from_utf8_lossy(&model[start..k]).to_string();
        let lower = word.to_ascii_lowercase();
        let ext_ok = [".mip", ".3do", ".pcx", ".tga"].iter().any(|e| lower.ends_with(e) && lower.len() > e.len());
        if ext_ok && !out.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
            out.push(word);
        }
    }
    out
}

/// What [`bring_in`] did with a .3do file.
#[derive(Debug, Default)]
pub struct BroughtIn {
    /// The name to put in the track.
    pub file: String,
    /// Files copied into the track folder.
    pub copied: Vec<String>,
    /// Files the object mentions that couldn't be found next to it.
    pub not_found: Vec<String>,
    /// A different file of the same name was already in the track folder and was kept.
    pub kept_existing: Vec<String>,
}

fn find_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file() && crate::util::file_name(p).eq_ignore_ascii_case(name))
}

/// Make object file `src` available to the track in `track_dir`: copy it (and the textures and
/// models it mentions, from its own folder) into the track folder, unless it's already there.
/// Never overwrites a file in the track folder. Installed tracks name their objects by bare
/// file name (never `shared\...`), so an object picked from the shared folder is copied too;
/// `shared_dir` only counts as "found" for files the object mentions.
pub fn bring_in(src: &Path, track_dir: &Path, shared_dir: Option<&Path>) -> Result<BroughtIn> {
    let name = crate::util::file_name(src).to_string();
    let mut out = BroughtIn { file: name.clone(), ..Default::default() };
    let src_dir = src.parent().context("the object file has no folder")?;
    let same = |a: &Path, b: &Path| {
        let (a, b) = (a.canonicalize().ok(), b.canonicalize().ok());
        a.is_some() && a == b
    };
    if same(src_dir, track_dir) {
        return Ok(out);
    }
    let mut todo = vec![name];
    let mut done: Vec<String> = Vec::new();
    while let Some(f) = todo.pop() {
        if done.iter().any(|d| d.eq_ignore_ascii_case(&f)) {
            continue;
        }
        done.push(f.clone());
        let Some(from) = find_in(src_dir, &f) else {
            let elsewhere = find_in(track_dir, &f).is_some() || shared_dir.is_some_and(|s| find_in(s, &f).is_some());
            if !elsewhere {
                out.not_found.push(f);
            }
            continue;
        };
        let bytes = std::fs::read(&from).with_context(|| format!("reading {}", from.display()))?;
        if crate::util::ext_lower(&from) == "3do" {
            todo.extend(files_used_by(&bytes));
        }
        match find_in(track_dir, &f) {
            Some(existing) => {
                if std::fs::read(&existing).map(|b| b != bytes).unwrap_or(true) {
                    out.kept_existing.push(f);
                }
            }
            None => {
                let to = track_dir.join(crate::util::file_name(&from));
                std::fs::write(&to, &bytes).with_context(|| format!("copying {} to {}", from.display(), to.display()))?;
                out.copied.push(crate::util::file_name(&to).to_string());
            }
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ locks

/// Locked objects can't be picked on the map, so they can't be moved by accident. Locks are
/// kept next to the track in `<track>.ptf.ttk.json` (the game ignores it), matched by file
/// name and position to the millimetre.
pub fn locks_path(track_file: &Path) -> PathBuf {
    let mut s = track_file.as_os_str().to_owned();
    s.push(".ttk.json");
    PathBuf::from(s)
}

#[derive(Serialize, Deserialize, Default)]
struct Sidecar {
    #[serde(default)]
    locked: Vec<LockKey>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
struct LockKey {
    file: String,
    x_mm: i64,
    y_mm: i64,
}

fn key(o: &super::model::Placed) -> LockKey {
    LockKey { file: o.file.to_ascii_lowercase(), x_mm: (o.x * 1000.0).round() as i64, y_mm: (o.y * 1000.0).round() as i64 }
}

/// The locked placements of a track (ids), from its sidecar file.
pub fn load_locks(track_file: &Path, t: &Track) -> HashSet<i32> {
    let Ok(text) = std::fs::read_to_string(locks_path(track_file)) else { return HashSet::new() };
    let Ok(s) = serde_json::from_str::<Sidecar>(&text) else { return HashSet::new() };
    let keys: HashSet<LockKey> = s.locked.into_iter().collect();
    t.objects.iter().filter(|o| keys.contains(&key(o))).map(|o| o.id).collect()
}

/// Write the sidecar (or remove it when nothing is locked).
pub fn save_locks(track_file: &Path, t: &Track, locked: &HashSet<i32>) -> Result<()> {
    let p = locks_path(track_file);
    let keys: Vec<LockKey> = t.objects.iter().filter(|o| locked.contains(&o.id)).map(key).collect();
    if keys.is_empty() {
        if p.is_file() {
            std::fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
        }
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&Sidecar { locked: keys })?;
    std::fs::write(&p, text).with_context(|| format!("writing {}", p.display()))
}
