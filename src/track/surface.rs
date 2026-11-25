//! Walls, ground strips and their textures.
//!
//! What the installed tracks show (checked on 40 game tracks, 4293 pieces):
//! - A piece lists its ground strips (F sections) and walls (W sections) in order of `dlat`,
//!   lowest first (right to left). New ones are put in that order too.
//! - Every strip and wall face has its own `AppearanceDescriptor` and `TextureCoordsDescriptor`
//!   (never shared), and the appearance points at a shared `TextureDescriptor` (the .mip file,
//!   always named with a path such as `trackmat\asphalt.mip`). So changing a face's texture
//!   only repoints its own appearance, and copying a strip or wall copies its appearances and
//!   texture coordinates but keeps sharing the textures.
//! - Which layer fields a strip/wall face has depends on which texture slots its appearance
//!   uses, so texture changes only swap the main slot (`tex[0]`) and never add or remove slots.

use super::edit::Change;
use super::graph::{Graph, Node};
use super::model::Track;
use super::objects::NodeEdit;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

/// Wall types to choose from, in plain words (the 2003 community list, see HANDOFF.md).
pub const WALL_TYPES: &[(i64, &str)] = &[
    (2049, "Hard wall"),
    (2050, "Very hard wall"),
    (2056, "Hard wall (bouncy)"),
    (2051, "Soft wall (springy)"),
    (2052, "Soft wall"),
    (2053, "Soft barrier"),
    (2054, "Hedge"),
    (2055, "Tire wall"),
    (2057, "Metal guard rail"),
    (2058, "Fence"),
    (2060, "Rubber wall"),
];

/// Ground surfaces to choose from (Sandbox's F-section surface numbers).
pub const SURFACES: &[(i64, &str)] = &[
    (1, "Asphalt"),
    (2, "Concrete"),
    (3, "Paint"),
    (4, "Grass"),
    (5, "Dirt"),
    (6, "Sand"),
    (7, "Gravel"),
    (8, "Water"),
    (12, "Rumble strip"),
];

/// Names of a wall's five faces, in file order.
pub const FACE_NAMES: [&str; 5] = ["Right side", "Top", "Left side", "Near end", "Far end"];

fn is_owned_class(class: &str) -> bool {
    class == "AppearanceDescriptor" || class == "TextureCoordsDescriptor"
}

/// Faces of a strip or wall that show a texture: (label, appearance id).
pub fn textured_faces(g: &Graph, id: i32) -> Vec<(String, i32)> {
    let Some(n) = g.node(id) else { return Vec::new() };
    let has_tex = |app: i32| g.node(app).and_then(|a| a.r("tex[0]")).is_some();
    let mut out = Vec::new();
    match n.class.as_str() {
        "F_SectionDescriptor" => {
            if let Some(a) = n.r("appearance").filter(|&a| has_tex(a)) {
                out.push(("Ground".to_string(), a));
            }
        }
        "W_SectionDescriptor" => {
            let subs = n.i("sub_count");
            for s in 0..subs {
                for (f, name) in FACE_NAMES.iter().enumerate() {
                    if let Some(a) = n.r(&format!("sub{s}.face{f}.appearance")).filter(|&a| has_tex(a)) {
                        let label = if subs > 1 { format!("{name} (part {})", s + 1) } else { name.to_string() };
                        out.push((label, a));
                    }
                }
            }
        }
        _ => {}
    }
    out
}

/// The main texture of an appearance: (texture id, file).
pub fn texture_of(g: &Graph, app: i32) -> Option<(i32, String)> {
    let t = g.node(app)?.r("tex[0]")?;
    Some((t, g.node(t)?.s("file").unwrap_or("").to_string()))
}

/// The textures strips and walls use, by file name: (texture id, file).
pub fn track_textures(g: &Graph) -> Vec<(i32, String)> {
    let mut by_file: HashMap<String, (i32, String)> = HashMap::new();
    for n in g.nodes.values().filter(|n| n.class == "F_SectionDescriptor" || n.class == "W_SectionDescriptor") {
        for (_, app) in textured_faces(g, n.id) {
            if let Some((t, f)) = texture_of(g, app) {
                let e = by_file.entry(f.to_ascii_lowercase()).or_insert((t, f));
                e.0 = e.0.min(t);
            }
        }
    }
    let mut v: Vec<(i32, String)> = by_file.into_values().collect();
    v.sort_by(|a, b| a.1.to_ascii_lowercase().cmp(&b.1.to_ascii_lowercase()));
    v
}

/// Copy a strip or wall with its own appearances and texture coordinates (textures stay shared).
fn deep_clone(g: &Graph, id: i32, next: &mut i32, out: &mut Vec<NodeEdit>) -> Result<i32> {
    let src = g.node(id).with_context(|| format!("object {id} is missing"))?;
    let mut n = src.clone();
    n.id = *next;
    *next += 1;
    for f in &mut n.fields {
        if f.1 != "obj" {
            continue;
        }
        let Some(c) = f.2.as_i64().map(|c| c as i32).filter(|&c| c != 0) else { continue };
        if g.node(c).is_some_and(|cn| is_owned_class(&cn.class)) {
            f.2 = json!(deep_clone(g, c, next, out)?);
        }
    }
    let new_id = n.id;
    out.push(NodeEdit { id: new_id, before: None, after: Some(n) });
    Ok(new_id)
}

/// A strip or wall and the appearances / texture coordinates that belong only to it.
fn owned(g: &Graph, id: i32, out: &mut Vec<i32>) {
    out.push(id);
    let Some(n) = g.node(id) else { return };
    for f in &n.fields {
        if f.1 == "obj" {
            if let Some(c) = f.2.as_i64().map(|c| c as i32).filter(|&c| c != 0) {
                if g.node(c).is_some_and(|cn| is_owned_class(&cn.class)) && !out.contains(&c) {
                    owned(g, c, out);
                }
            }
        }
    }
}

fn mid_dlat(n: &Node) -> f64 {
    0.5 * (n.f("dlat0") + n.f("dlat1"))
}

/// The piece's list `stem` with `new` added in dlat order (lowest first, like the stock files).
fn sorted_with(g: &Graph, seg: &Node, stem: &str, new: &Node) -> Vec<i32> {
    let mut ids = seg.list_ids(stem);
    let key = |id: i32| if id == new.id { mid_dlat(new) } else { g.node(id).map_or(0.0, mid_dlat) };
    ids.push(new.id);
    ids.sort_by(|a, b| key(*a).total_cmp(&key(*b)));
    ids
}

fn segment<'a>(g: &'a Graph, t: &Track, k: usize) -> Result<&'a Node> {
    let s = t.segments.get(k).context("no such piece")?;
    g.node(s.id).context("the piece is missing")
}

fn node_after(edits: &[NodeEdit], id: i32) -> Option<&Node> {
    edits.iter().find(|e| e.id == id).and_then(|e| e.after.as_ref())
}

fn set_in(edits: &mut [NodeEdit], id: i32, field: &str, v: Value) -> Result<()> {
    let n = edits.iter_mut().find(|e| e.id == id).and_then(|e| e.after.as_mut()).context("new object missing")?;
    n.set(field, v)?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
}

/// Add a wall to piece `k` on one side, copied from the nearest wall on that side (this piece
/// first, then the closest piece; any wall if none is on that side). It sits on the outer edge
/// of the ground. Returns the edits and the new wall's id.
pub fn add_wall(g: &Graph, t: &Track, k: usize, side: Side) -> Result<(Vec<NodeEdit>, i32)> {
    let seg = segment(g, t, k)?;
    let sign = if side == Side::Left { 1.0 } else { -1.0 };
    let n = t.segments.len();
    let mut template = None;
    'search: for dist in 0..n {
        for kk in [(k + dist) % n, (k + n - dist % n) % n] {
            let ws = &t.segments[kk].ws;
            if let Some(w) = ws.iter().filter(|w| sign * (w.dlat[0] + w.dlat[1]) > 0.0).max_by(|a, b| {
                (sign * (a.dlat[0] + a.dlat[1])).total_cmp(&(sign * (b.dlat[0] + b.dlat[1])))
            }) {
                template = Some(w.id);
                break 'search;
            }
        }
    }
    let template = template
        .or_else(|| t.segments.iter().flat_map(|s| s.ws.iter()).next().map(|w| w.id))
        .context("this track has no walls to copy from")?;
    let mut next = g.next_id();
    let mut edits = Vec::new();
    let id = deep_clone(g, template, &mut next, &mut edits)?;
    let s = &t.segments[k];
    let w = node_after(&edits, id).context("copy failed")?;
    let half = [0.5 * w.f("width0"), 0.5 * w.f("width1")];
    let edge = |e: usize| {
        let t = e as f64;
        if side == Side::Left {
            s.max_dlat(t) - half[e]
        } else {
            s.min_dlat(t) + half[e]
        }
    };
    let (d0, d1) = (edge(0), edge(1));
    set_in(&mut edits, id, "dlat0", json!(d0))?;
    set_in(&mut edits, id, "dlat1", json!(d1))?;
    let new = node_after(&edits, id).context("copy failed")?.clone();
    let ids = sorted_with(g, seg, "w", &new);
    edits.push(NodeEdit { id: seg.id, before: Some(seg.clone()), after: Some(seg.with_list("w", &ids)?) });
    Ok((edits, id))
}

/// After wall `id` of piece `k` moves to middle distance `new_mid`, the piece's wall list in
/// dlat order (the stock files always keep it so), as an edit, if the order changed.
pub fn resort_walls(g: &Graph, t: &Track, k: usize, id: i32, new_mid: f64) -> Option<NodeEdit> {
    let seg = segment(g, t, k).ok()?;
    let ids = seg.list_ids("w");
    let key = |i: i32| if i == id { new_mid } else { g.node(i).map_or(0.0, mid_dlat) };
    let mut sorted = ids.clone();
    sorted.sort_by(|a, b| key(*a).total_cmp(&key(*b)));
    if sorted == ids {
        return None;
    }
    Some(NodeEdit { id: seg.id, before: Some(seg.clone()), after: Some(seg.with_list("w", &sorted).ok()?) })
}

/// Remove a wall or ground strip from piece `k`. The strip to its right then reaches further left.
pub fn delete_part(g: &Graph, t: &Track, k: usize, id: i32) -> Result<Vec<NodeEdit>> {
    let seg = segment(g, t, k)?;
    let stem = match g.node(id).map(|n| n.class.as_str()) {
        Some("W_SectionDescriptor") => "w",
        Some("F_SectionDescriptor") => "f",
        _ => bail!("that isn't a wall or a ground strip"),
    };
    let ids: Vec<i32> = seg.list_ids(stem).into_iter().filter(|&i| i != id).collect();
    if stem == "f" && ids.is_empty() {
        bail!("a piece needs at least one ground strip");
    }
    let mut edits = vec![NodeEdit { id: seg.id, before: Some(seg.clone()), after: Some(seg.with_list(stem, &ids)?) }];
    let mut gone = Vec::new();
    owned(g, id, &mut gone);
    for o in gone {
        if let Some(n) = g.node(o) {
            edits.push(NodeEdit { id: o, before: Some(n.clone()), after: None });
        }
    }
    Ok(edits)
}

/// Split a ground strip in two down the middle (both halves keep its surface and texture).
/// Returns the edits and the new strip (the left half).
pub fn split_strip(g: &Graph, t: &Track, k: usize, id: i32) -> Result<(Vec<NodeEdit>, i32)> {
    let seg = segment(g, t, k)?;
    let s = &t.segments[k];
    let f = g.node(id).filter(|n| n.class == "F_SectionDescriptor").context("that isn't a ground strip")?;
    // The strip runs left from this F to the next F to the left (or the left edge of the ground).
    let ends = |e: usize| -> (f64, f64) {
        let d = f.f(&format!("dlat{e}"));
        let to = s
            .fs
            .iter()
            .map(|x| x.dlat[e])
            .filter(|&x| x > d + 1e-6)
            .fold(f64::MAX, f64::min);
        let to = if to == f64::MAX { s.max_dlat(e as f64) } else { to };
        (d, to)
    };
    let (a0, b0) = ends(0);
    let (a1, b1) = ends(1);
    if b0 - a0 < 0.2 || b1 - a1 < 0.2 {
        bail!("this strip is too narrow to split");
    }
    let mut next = g.next_id();
    let mut edits = Vec::new();
    let new_id = deep_clone(g, id, &mut next, &mut edits)?;
    set_in(&mut edits, new_id, "dlat0", json!(0.5 * (a0 + b0)))?;
    set_in(&mut edits, new_id, "dlat1", json!(0.5 * (a1 + b1)))?;
    let new = node_after(&edits, new_id).context("copy failed")?.clone();
    let ids = sorted_with(g, seg, "f", &new);
    edits.push(NodeEdit { id: seg.id, before: Some(seg.clone()), after: Some(seg.with_list("f", &ids)?) });
    Ok((edits, new_id))
}

/// How far a strip's right edge (its own dlat) may move at end `e` (0 start, 1 end) without
/// passing the strips beside it.
pub fn strip_edge_limits(t: &Track, k: usize, id: i32, e: usize) -> (f64, f64) {
    let s = &t.segments[k];
    let Some(me) = s.fs.iter().find(|f| f.id == id) else { return (-1000.0, 1000.0) };
    let d = me.dlat[e];
    let lo = s.fs.iter().map(|f| f.dlat[e]).filter(|&x| x < d - 1e-9).fold(-1000.0, f64::max);
    let hi = s.fs.iter().map(|f| f.dlat[e]).filter(|&x| x > d + 1e-9).fold(1000.0, f64::min);
    (lo + 0.05, hi - 0.05)
}

/// Field changes that set a wall's thickness, keeping its physical width (`d35`/`d3d`) the same
/// amount wider as before (stock walls have them equal, or 0.1 m wider).
pub fn wall_width(g: &Graph, id: i32, w0: f64, w1: f64) -> Vec<Change> {
    let Some(n) = g.node(id) else { return Vec::new() };
    let c = |f: &str, v: f64| Change { id, field: f.into(), value: json!(v) };
    vec![
        c("width0", w0),
        c("width1", w1),
        c("d35", w0 + (n.f("d35") - n.f("width0"))),
        c("d3d", w1 + (n.f("d3d") - n.f("width1"))),
    ]
}

/// Point appearances at texture `tex`.
/// Every strip and wall face in the track showing `file` (any case): their appearance ids.
pub fn faces_using(g: &Graph, file: &str) -> Vec<i32> {
    let mut apps: Vec<i32> = g
        .nodes
        .values()
        .filter(|n| n.class == "F_SectionDescriptor" || n.class == "W_SectionDescriptor")
        .flat_map(|n| textured_faces(g, n.id))
        .map(|(_, app)| app)
        .filter(|&app| texture_of(g, app).is_some_and(|(_, f)| f.eq_ignore_ascii_case(file)))
        .collect();
    apps.sort_unstable();
    apps.dedup();
    apps
}

pub fn set_texture(apps: &[i32], tex: i32) -> Vec<Change> {
    apps.iter().map(|&a| Change { id: a, field: "tex[0]".into(), value: json!(tex) }).collect()
}

/// The texture for `file`: the existing one (any case), or a new one to add.
pub fn texture_for_file(g: &Graph, file: &str) -> (Option<NodeEdit>, i32) {
    let mut found: Vec<i32> = g
        .nodes
        .values()
        .filter(|n| n.class == "TextureDescriptor" && n.s("file").is_some_and(|f| f.eq_ignore_ascii_case(file)))
        .map(|n| n.id)
        .collect();
    found.sort_unstable();
    if let Some(&id) = found.first() {
        return (None, id);
    }
    let id = g.next_id();
    let f = |n: &str, k: &str, v: Value| (n.to_string(), k.to_string(), v);
    let node = Node {
        id,
        class: "TextureDescriptor".into(),
        fields: vec![
            f("base", "i32", json!(1)),
            f("name", "str", Value::Null),
            f("version", "i32", json!(2)),
            f("file", "str", json!(file)),
            f("flag", "u8", json!(0)),
        ],
    };
    (Some(NodeEdit { id, before: None, after: Some(node) }), id)
}

/// Make a .mip file available to the track: the name to use for it, and whether it was copied.
/// Files already in the track folder (or its `trackmat` folder) are used where they are;
/// others are copied into `trackmat\`, never over a file that's already there.
/// Picture files [`picture_to_mip`] reads.
pub const PICTURE_TYPES: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tga"];

/// Turn a picture (PNG, JPEG, WebP, BMP or TGA) into a ground texture in the track's `trackmat`
/// folder, with Papyrus's makemip from `sandbox_dir`: powers of two with the longer side at most
/// 1024 px, DXT, tiling, like the stock ground textures. A file of the same name is never
/// overwritten (the new one gets `_2`, `_3`...). Returns the name as the track refers to it
/// (`trackmat\<name>.mip`) and plain notes about what was done to the picture.
pub fn picture_to_mip(sandbox_dir: &Path, picture: &Path, track_dir: &Path) -> Result<(String, Vec<String>)> {
    use crate::mip::build::{build_image, safe_stem, Recipe};
    if !sandbox_dir.join("makemip.exe").is_file() {
        bail!("makemip.exe wasn't found in {} (set the Sandbox folder in Settings)", sandbox_dir.display());
    }
    let img = image::open(picture)
        .with_context(|| format!("can't read {} as a picture", crate::util::file_name(picture)))?
        .to_rgba8();
    let tm = track_dir.join("trackmat");
    std::fs::create_dir_all(&tm).with_context(|| format!("creating {}", tm.display()))?;
    let stem = picture.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let stem = safe_stem(&stem).to_ascii_lowercase();
    let mut name = format!("{stem}.mip");
    let mut k = 2;
    while tm.join(&name).exists() {
        name = format!("{stem}_{k}.mip");
        k += 1;
    }
    let mut recipe = Recipe::new_texture(true, false);
    recipe.size = Some(crate::acscenery::mip_size(img.width(), img.height(), 1024));
    let work = tm.join("_ttk_work");
    let built = build_image(sandbox_dir, img, &tm.join(&name), &recipe, &work);
    let _ = std::fs::remove_dir_all(&work);
    let built = built?;
    Ok((format!("trackmat\\{name}"), built.notes))
}

pub fn bring_in_texture(src: &Path, track_dir: &Path) -> Result<(String, Option<String>)> {
    let name = crate::util::file_name(src).to_string();
    let parent = src.parent().and_then(|p| p.canonicalize().ok());
    let tm = track_dir.join("trackmat");
    if parent.is_some() && parent == track_dir.canonicalize().ok() {
        return Ok((name, None));
    }
    if parent.is_some() && parent == tm.canonicalize().ok() {
        return Ok((format!("trackmat\\{name}"), None));
    }
    std::fs::create_dir_all(&tm).with_context(|| format!("creating {}", tm.display()))?;
    let to = tm.join(&name);
    if to.is_file() {
        let same = std::fs::read(&to).ok() == std::fs::read(src).ok();
        let note = (!same).then(|| format!("trackmat already had a different {name}; that one is used"));
        return Ok((format!("trackmat\\{name}"), note));
    }
    std::fs::copy(src, &to).with_context(|| format!("copying {} to {}", src.display(), to.display()))?;
    Ok((format!("trackmat\\{name}"), Some(format!("copied {name} into trackmat"))))
}

/// Give the walls on the same side of pieces `pieces` the type, height, thickness and textures
/// of wall `from`. In each piece the wall nearest to the same distance from the centre is used.
/// Returns the changes and how many pieces had no wall on that side.
pub fn copy_wall_to(g: &Graph, t: &Track, from: i32, pieces: std::ops::RangeInclusive<usize>) -> (Vec<Change>, usize) {
    let Some(src) = g.node(from) else { return (Vec::new(), 0) };
    let d = mid_dlat(src);
    let src_faces = textured_faces(g, from);
    let mut out = Vec::new();
    let mut missing = 0;
    for k in pieces {
        let Some(s) = t.segments.get(k) else { continue };
        let Some(w) = s
            .ws
            .iter()
            .filter(|w| w.id != from && (w.dlat[0] + w.dlat[1]) * d > 0.0)
            .min_by(|a, b| (0.5 * (a.dlat[0] + a.dlat[1]) - d).abs().total_cmp(&(0.5 * (b.dlat[0] + b.dlat[1]) - d).abs()))
        else {
            if !s.ws.iter().any(|w| w.id == from) {
                missing += 1;
            }
            continue;
        };
        let c = |f: &str| Change { id: w.id, field: f.into(), value: src.get(f).cloned().unwrap_or(Value::Null) };
        out.extend(["wall_type", "height0", "height1"].map(c));
        out.extend(wall_width(g, w.id, src.f("width0"), src.f("width1")));
        // matching faces take the same texture
        for (label, app) in textured_faces(g, w.id) {
            if let Some((_, sa)) = src_faces.iter().find(|(l, _)| *l == label) {
                if let Some((tex, _)) = texture_of(g, *sa) {
                    out.extend(set_texture(&[app], tex));
                }
            }
        }
    }
    (out, missing)
}

/// Give the strip at the same distance from the centre in pieces `pieces` the surface and
/// texture of strip `from`.
pub fn copy_strip_to(g: &Graph, t: &Track, from: i32, pieces: std::ops::RangeInclusive<usize>) -> Vec<Change> {
    let Some(src) = g.node(from) else { return Vec::new() };
    let tex = textured_faces(g, from).first().and_then(|(_, a)| texture_of(g, *a)).map(|x| x.0);
    // middle of the source strip
    let src_seg = t.segments.iter().find(|s| s.fs.iter().any(|f| f.id == from));
    let d = src_seg.map_or(mid_dlat(src), |s| {
        let a = mid_dlat(src);
        let b = s.fs.iter().map(|f| 0.5 * (f.dlat[0] + f.dlat[1])).filter(|&x| x > a + 1e-6).fold(f64::MAX, f64::min);
        if b == f64::MAX { a + 0.5 } else { 0.5 * (a + b) }
    });
    let mut out = Vec::new();
    for k in pieces {
        let Some(s) = t.segments.get(k) else { continue };
        // the strip whose right edge is the last one at or right of d
        let Some(f) = s
            .fs
            .iter()
            .filter(|f| 0.5 * (f.dlat[0] + f.dlat[1]) <= d)
            .max_by(|a, b| (a.dlat[0] + a.dlat[1]).total_cmp(&(b.dlat[0] + b.dlat[1])))
        else {
            continue;
        };
        if f.id == from {
            continue;
        }
        out.push(Change { id: f.id, field: "i15".into(), value: src.get("i15").cloned().unwrap_or(json!(1)) });
        if let (Some(tex), Some((_, app))) = (tex, textured_faces(g, f.id).first()) {
            out.extend(set_texture(&[*app], tex));
        }
    }
    out
}
