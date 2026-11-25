//! Assetto Corsa scenery (buildings, stands, trees, fences, the land beyond the road) as NR2003
//! trackside objects.
//!
//! What becomes scenery: every visible AC mesh that isn't a driving surface or a wall (those are
//! the converted track itself). Triangles lying on NR2003's own ground (AC's visual road, kerb and
//! grass layers) are dropped, because NR2003 draws that ground itself.
//!
//! The rest is cut into map squares (`cell_m`). Each square becomes one .3do built by Papyrus's
//! make3do: one shape per material, split below make3do's limits, under LODs that drop the
//! square's big things beyond `lod_m` and its small things beyond `detail_lod_m`. Each square is
//! placed in the track once, at its centre, facing 0.
//!
//! make3do's limits per shape (measured 2026-11-24): 32767 faces (more is a syntax error at
//! FACE_LIST) and 16536 vertices (16538 gives "Could not write out 3do file"). Several shapes in
//! one .3do are fine (3 x 24200 faces built).
//!
//! Textures become .mip files through makemip (DXT, tiling): opaque materials lose their alpha
//! (AC keeps other data there), cut-outs (trees, fences) get hard edges and their faces both ways.

use crate::ac::AcScene;
use crate::logger::Logger;
use crate::mip::build::{build_image, safe_stem, Family, Recipe};
use crate::runner::run_tool;
use crate::track::graph::Graph;
use crate::track::model::{Segment, Track};
use crate::track::objects::{self, NewObject};
use anyhow::{bail, Context, Result};
use image::RgbaImage;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// The ground texture's file in the track (copied from the template, see `Options::ground_texture`).
pub const GROUND_MIP: &str = "ac_ground.mip";

/// The texture the converted track's grass strips use most (as the track names it).
pub fn grass_texture(graph: &Graph) -> Option<String> {
    let t = Track::from_graph(graph).ok()?;
    let mut count: HashMap<String, usize> = HashMap::new();
    for s in &t.segments {
        for f in s.fs.iter().filter(|f| f.surface == 4) {
            if let Some(tx) = &f.tex {
                *count.entry(tx.file.clone()).or_default() += 1;
            }
        }
    }
    count.into_iter().max_by_key(|(_, n)| *n).map(|(f, _)| f)
}

/// Most triangles in one object. make3do built squares of about 200,000 triangles but failed
/// ("Could not write out 3do file") on Road America's densest, about 1 million (its tyre walls,
/// 1.6 million triangles in all); heavier squares are split into quarters.
pub const MAX_CELL_TRIANGLES: usize = 250_000;

/// Split a square into quarters (by where each triangle's middle is), again and again, until
/// every part has at most `max` triangles. Parts keep the square's centre and get `_0`.. `_3`.
fn split_heavy(cell: Cell, max: usize) -> Vec<Cell> {
    if cell.triangles() <= max {
        return vec![cell];
    }
    let mut parts: Vec<Vec<Shape>> = vec![Vec::new(); 4];
    let mut moved = false;
    for sh in &cell.shapes {
        // per quarter: a new shape with the vertices it uses
        let mut q_shapes: Vec<Shape> = (0..4).map(|_| Shape { material: sh.material, small: sh.small, ..Default::default() }).collect();
        let mut maps: Vec<HashMap<u32, u32>> = vec![HashMap::new(); 4];
        for t in &sh.tris {
            let c: [f32; 2] = std::array::from_fn(|k| (sh.verts[t[0] as usize][k] + sh.verts[t[1] as usize][k] + sh.verts[t[2] as usize][k]) / 3.0);
            let q = (c[0] >= 0.0) as usize + 2 * (c[1] >= 0.0) as usize;
            let mut idx = [0u32; 3];
            for (k, &vi) in t.iter().enumerate() {
                idx[k] = *maps[q].entry(vi).or_insert_with(|| {
                    q_shapes[q].verts.push(sh.verts[vi as usize]);
                    (q_shapes[q].verts.len() - 1) as u32
                });
            }
            q_shapes[q].tris.push(idx);
        }
        for (q, qs) in q_shapes.into_iter().enumerate() {
            if !qs.tris.is_empty() {
                parts[q].push(qs);
            }
        }
    }
    let total = cell.triangles();
    for p in &parts {
        moved |= p.iter().map(|s| s.tris.len()).sum::<usize>() < total;
    }
    if !moved {
        return vec![cell]; // everything sits in one quarter; splitting can't help
    }
    let mut out = Vec::new();
    for (q, shapes) in parts.into_iter().enumerate() {
        if shapes.is_empty() {
            continue;
        }
        // re-centre the quarter so its own quarters split it again
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for sh in &shapes {
            for v in &sh.verts {
                for k in 0..2 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
        }
        let mid = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
        let shapes: Vec<Shape> = shapes
            .into_iter()
            .map(|mut sh| {
                for v in &mut sh.verts {
                    v[0] -= mid[0];
                    v[1] -= mid[1];
                }
                sh
            })
            .collect();
        let part = Cell {
            name: format!("{}_{q}", cell.name),
            centre: [cell.centre[0] + mid[0] as f64, cell.centre[1] + mid[1] as f64, cell.centre[2]],
            shapes,
        };
        out.extend(split_heavy(part, max));
    }
    out
}

/// Most faces in one shape (make3do fails at 32768).
pub const MAX_SHAPE_FACES: usize = 30_000;
/// Most vertices in one shape.
// A well-joined grid builds up to 16,536 vertices, but meshes of loose triangles fail sooner:
// Road America's bleachers (16,000 vertices, 8,240 faces) gave "Could not write out 3do file",
// as did 16,998 loose vertices (5,666 faces). make3do's limit is on its triangle-strip output,
// so shapes stay well below.
pub const MAX_SHAPE_VERTS: usize = 10_000;

#[derive(Clone, Debug)]
pub struct Options {
    /// Size of the map squares, each its own object (metres).
    pub cell_m: f64,
    /// A square isn't drawn beyond this distance (metres; NR2003's detail slider scales it).
    pub lod_m: f64,
    /// Small things (under `small_m` across: cones, people, chairs) aren't drawn beyond this.
    pub detail_lod_m: f64,
    pub small_m: f64,
    /// Largest texture side (pixels, a power of two).
    pub tex_side: u32,
    /// Show trees, fences and other cut-outs from both sides.
    pub two_sided_cutouts: bool,
    /// Triangles up to this far above NR2003's ground, facing up, count as lying on it (metres).
    /// 0.5 m left AC's visual road and grass layers (Road America's VISROAD, VISGRASS, groove)
    /// floating over NR2003's road, smeared with their blend-map textures.
    pub on_ground_m: f64,
    /// Keep the .pas/.psg files make3do was given (for editing in Blender).
    pub keep_pas: bool,
    /// Most object files (squares). Installed tracks use at most 299 different object files
    /// (Charlotte_2018_BBMC_Day; Las Vegas 255), so the squares grow until there are at most
    /// this many.
    pub max_objects: usize,
    /// Most triangles in one object; heavier squares are split into quarters.
    pub max_object_triangles: usize,
    /// Most scenery triangles in all (0 = no limit). NR2003 crashed with Road America's 3.4
    /// million (also when cut into 646 small objects) and with 1.4 million; it loaded about
    /// 600 thousand (tested 2026-11-24). Dense meshes are simplified (close vertices merged)
    /// until the scenery fits.
    pub max_triangles: usize,
    /// Merge distance (metres; 0 = keep all), scaled for each mesh by how dense it is (see
    /// `mesh_merge`). Set by `plan` to meet `max_triangles`.
    pub merge_m: f64,
    /// AC meshes left out to meet `max_triangles` (set by `plan`).
    pub left_out: std::collections::HashSet<usize>,
    /// For land whose AC texture is only a blurry colour map: a ground texture of the template
    /// track (its folder, and the file as its grass strips name it), tiled by map position.
    pub ground_texture: Option<(PathBuf, String)>,
}

impl Default for Options {
    fn default() -> Self {
        Self { cell_m: 150.0, lod_m: 800.0, detail_lod_m: 300.0, small_m: 8.0, tex_side: 512, two_sided_cutouts: true, on_ground_m: 1.5, keep_pas: false, max_objects: 250, max_object_triangles: MAX_CELL_TRIANGLES, max_triangles: 600_000, merge_m: 0.0, left_out: Default::default(), ground_texture: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Alpha {
    Opaque,
    Cutout,
    Blend,
}

/// A material as NR2003 will see it.
#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    /// The .mip it shows, if any.
    pub mip: Option<String>,
    pub alpha: Alpha,
    /// Texture coordinates are multiplied by this (multi-layer materials' detail repeat).
    pub uv_scale: f32,
    /// Texture coordinates from map position instead, one repeat per this many metres (0 = off).
    pub planar_m: f32,
}

/// A texture to make: AC texture index, .mip name, and how its alpha is used.
#[derive(Clone, Debug)]
pub struct TextureJob {
    pub texture: usize,
    pub source_name: String,
    pub mip: String,
    pub alpha: Alpha,
}

/// One shape: vertices are (x, y, z relative to the square's centre, normal, u, v as PAS wants).
#[derive(Clone, Debug, Default)]
pub struct Shape {
    pub material: usize,
    /// Part of a small thing (drawn to `detail_lod_m` only).
    pub small: bool,
    pub verts: Vec<[f32; 8]>,
    pub tris: Vec<[u32; 3]>,
}

/// One map square: one .3do.
#[derive(Clone, Debug)]
pub struct Cell {
    /// File stem (`sc_3_12`).
    pub name: String,
    /// Where it's placed (NR2003 world).
    pub centre: [f64; 3],
    pub shapes: Vec<Shape>,
}

impl Cell {
    pub fn triangles(&self) -> usize {
        self.shapes.iter().map(|s| s.tris.len()).sum()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// AC meshes used as scenery.
    pub meshes: usize,
    /// Left out: driving surfaces and walls (the track itself), hidden meshes, AC's 3D grass
    /// (NR2003's grass ground stands in for it).
    pub meshes_track: usize,
    pub meshes_hidden: usize,
    pub meshes_grass: usize,
    /// Of `meshes`, the small things.
    pub meshes_small: usize,
    /// Triangles kept (cut-outs' back faces included).
    pub triangles: usize,
    /// Triangles dropped for lying on NR2003's own ground.
    pub on_ground: usize,
    /// Triangles with no area.
    pub degenerate: usize,
    /// Size of the squares used (metres): `cell_m`, or bigger to stay within `max_objects`.
    pub cell_m: f64,
    /// Scenery triangles before simplifying, and the merge distance used (0 = none needed).
    pub triangles_before: usize,
    pub merge_m: f64,
    /// AC meshes left out to stay within the budget (the densest first).
    pub meshes_left_out: usize,
}

pub struct Plan {
    pub cells: Vec<Cell>,
    pub materials: Vec<Material>,
    pub textures: Vec<TextureJob>,
    pub stats: Stats,
}

/// NR2003's ground, for finding AC triangles that lie on it.
struct NrGround<'a> {
    segs: &'a [Segment],
    bins: HashMap<(i32, i32), Vec<usize>>,
    bin: f64,
}

impl<'a> NrGround<'a> {
    fn new(t: &'a Track) -> Self {
        let bin = 25.0;
        let mut bins: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (k, s) in t.segments.iter().enumerate() {
            let steps = ((s.length / 5.0).ceil() as usize).clamp(2, 400);
            let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
            for i in 0..=steps {
                let tt = i as f64 / steps as f64;
                for d in [s.min_dlat(tt) - 1.0, 0.0, s.max_dlat(tt) + 1.0] {
                    let p = s.point(tt, d);
                    for c in 0..2 {
                        lo[c] = lo[c].min(p[c]);
                        hi[c] = hi[c].max(p[c]);
                    }
                }
            }
            for i in (lo[0] / bin).floor() as i32..=(hi[0] / bin).floor() as i32 {
                for j in (lo[1] / bin).floor() as i32..=(hi[1] / bin).floor() as i32 {
                    bins.entry((i, j)).or_default().push(k);
                }
            }
        }
        Self { segs: &t.segments, bins, bin }
    }

    /// NR2003's ground height under (x, y), if NR2003 has ground there.
    fn height(&self, x: f64, y: f64) -> Option<f64> {
        let key = ((x / self.bin).floor() as i32, (y / self.bin).floor() as i32);
        let mut best: Option<f64> = None;
        for &k in self.bins.get(&key)? {
            let s = &self.segs[k];
            let (t, d) = s.locate(x, y);
            if !(-1e-6..=1.0 + 1e-6).contains(&t) {
                continue;
            }
            let t = t.clamp(0.0, 1.0);
            if d < s.min_dlat(t) - 0.05 || d > s.max_dlat(t) + 0.05 {
                continue;
            }
            let h = s.height(t, d);
            best = Some(best.map_or(h, |b: f64| b.max(h)));
        }
        best
    }
}

fn alpha_of(m: &crate::ac::AcMaterial) -> Alpha {
    if m.blend != 0 {
        Alpha::Blend
    } else if m.alpha_test {
        Alpha::Cutout
    } else {
        Alpha::Opaque
    }
}

/// Name for a texture's .mip: `ac_<stem>.mip`, at most 30 characters, unique (any case).
fn mip_name(source: &str, alpha: bool, taken: &mut Vec<String>) -> String {
    let stem = Path::new(source).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut stem: String = safe_stem(&stem).to_ascii_lowercase().chars().take(20).collect();
    if alpha {
        stem.push_str("_a");
    }
    let mut name = format!("ac_{stem}.mip");
    let mut k = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
        name = format!("ac_{stem}{k}.mip");
        k += 1;
    }
    taken.push(name.clone());
    name
}

/// Which AC triangles become scenery, cut into map squares (at least `cell_m`, bigger if that
/// would make more than `max_objects` objects). `offset` and `height_base` are the converter's
/// shifts from AC's coordinates to the NR2003 track's (`acconv::Report`).
pub fn plan(scene: &AcScene, track: &Track, offset: [f64; 2], height_base: f64, opt: &Options) -> Plan {
    let mut o = opt.clone();
    // the triangle budget: the smallest merge distance that fits (counting is quick; the
    // triangles dropped for lying on NR2003's ground come off later, so this errs low)
    let before = count_triangles(scene, opt, 0.0);
    if opt.max_triangles > 0 && before > opt.max_triangles {
        let (mut lo, mut hi) = (0.005f64, 0.005f64);
        while count_triangles(scene, opt, hi) > opt.max_triangles && hi < 50.0 {
            lo = hi;
            hi *= 2.0;
        }
        for _ in 0..10 {
            let mid = (lo * hi).sqrt();
            if count_triangles(scene, opt, mid) > opt.max_triangles {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        o.merge_m = hi;
        // merging is capped (MAX_MERGE_M): if that isn't enough, leave the densest meshes out
        // first (tyre stacks and the like: few meshes, most of the triangles)
        let mut total = count_triangles(scene, &o, o.merge_m);
        if total > opt.max_triangles {
            let mut order: Vec<(usize, bool, f64, usize)> = scene
                .meshes
                .iter()
                .enumerate()
                .filter(|(_, m)| is_scenery(scene, m))
                .map(|(i, m)| (i, is_small(m, opt.small_m), density(m), mesh_cost(scene, &o, m, o.merge_m)))
                .collect();
            order.sort_by(|a, b| b.2.total_cmp(&a.2));
            if std::env::var_os("TTK_SCENERY_DEBUG").is_some() {
                let mut by_cost = order.clone();
                by_cost.sort_by(|a, b| b.3.cmp(&a.3));
                eprintln!("[scenery] {} triangles after merging (budget {}); costliest meshes:", total, opt.max_triangles);
                for (i, small, d, cost) in by_cost.iter().take(12) {
                    let m = &scene.meshes[*i];
                    eprintln!("  {:>8} of {:>8}  density {:>8.1}/m2  small {}  {}", cost, m.indices.len() / 3, d, small, m.name);
                }
                let n_small = order.iter().filter(|o| o.1).count();
                eprintln!("  {} meshes, {} small; total of the 1000 densest: {}", order.len(), n_small, order.iter().take(1000).map(|o| o.3).sum::<usize>());
            }
            for (i, _, _, cost) in order {
                if total <= opt.max_triangles {
                    break;
                }
                o.left_out.insert(i);
                total -= cost.min(total);
            }
        }
    }
    loop {
        let mut p = plan_cells(scene, track, offset, height_base, &o);
        p.stats.triangles_before = before;
        p.stats.merge_m = o.merge_m;
        p.stats.meshes_left_out = o.left_out.len();
        let n = p.cells.len();
        if n <= opt.max_objects.max(1) || o.cell_m >= 3000.0 {
            // squares too heavy for make3do are split into quarters until they aren't
            p.cells = p.cells.into_iter().flat_map(|c| split_heavy(c, opt.max_object_triangles.max(1000))).collect();
            return p;
        }
        // squares' count goes roughly with 1 / size²
        o.cell_m = (o.cell_m * (n as f64 / opt.max_objects.max(1) as f64).sqrt() * 1.05).min(3000.0);
    }
}

/// Is this mesh scenery (not the track, hidden, or AC's 3D grass)?
fn is_scenery(scene: &AcScene, mesh: &crate::ac::AcMesh) -> bool {
    let shader = mesh.material.and_then(|m| scene.materials.get(m)).map_or("", |m| m.shader.as_str());
    mesh.surface.is_none() && mesh.visible && !shader.eq_ignore_ascii_case("ksGrass")
}

/// A mesh's triangles with vertices closer than `merge_m` merged: each group becomes one
/// vertex at the group's middle (taking the first one's normal and texture position), and
/// triangles that collapse, and repeats, are dropped. Vertices facing different ways (another
/// normal octant) stay apart, so corners keep their shape. Returns the triangles and, when
/// merging, the mesh's vertices with the groups moved to their middles.
fn merged_triangles(mesh: &crate::ac::AcMesh, merge_m: f64) -> (Vec<[u32; 3]>, Option<Vec<f32>>) {
    let tris = mesh.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]);
    if merge_m <= 0.0 {
        return (tris.collect(), None);
    }
    let nv = mesh.verts.len() / 8;
    let mut first: HashMap<(i32, i32, i32, u8), u32> = HashMap::with_capacity(nv);
    let rep: Vec<u32> = (0..nv)
        .map(|i| {
            let v = &mesh.verts[i * 8..i * 8 + 8];
            let q = |x: f32| (x as f64 / merge_m).floor() as i32;
            let oct = (v[3] >= 0.0) as u8 | ((v[4] >= 0.0) as u8) << 1 | ((v[5] >= 0.0) as u8) << 2;
            *first.entry((q(v[0]), q(v[1]), q(v[2]), oct)).or_insert(i as u32)
        })
        .collect();
    // each group's middle
    let mut sum: HashMap<u32, ([f64; 3], usize)> = HashMap::new();
    for (i, &r) in rep.iter().enumerate() {
        let e = sum.entry(r).or_insert(([0.0; 3], 0));
        for c in 0..3 {
            e.0[c] += mesh.verts[i * 8 + c] as f64;
        }
        e.1 += 1;
    }
    let mut verts = mesh.verts.clone();
    for (r, (acc, n)) in sum {
        if n > 1 {
            for c in 0..3 {
                verts[r as usize * 8 + c] = (acc[c] / n as f64) as f32;
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    let tris = tris
        .filter_map(|t| {
            let m = [rep[t[0] as usize], rep[t[1] as usize], rep[t[2] as usize]];
            if m[0] == m[1] || m[1] == m[2] || m[0] == m[2] {
                return None;
            }
            let mut k = m;
            k.sort_unstable();
            seen.insert(k).then_some(m)
        })
        .collect();
    (tris, Some(verts))
}

/// Under `small_m` across (cones, people, chairs).
fn is_small(mesh: &crate::ac::AcMesh, small_m: f64) -> bool {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in mesh.verts.chunks_exact(8) {
        for c in 0..3 {
            lo[c] = lo[c].min(v[c]);
            hi[c] = hi[c].max(v[c]);
        }
    }
    let d = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
    (d as f64) < small_m
}

/// Triangles per square metre of a mesh (over the area of its box's biggest side).
fn density(mesh: &crate::ac::AcMesh) -> f64 {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in mesh.verts.chunks_exact(8) {
        for c in 0..3 {
            lo[c] = lo[c].min(v[c]);
            hi[c] = hi[c].max(v[c]);
        }
    }
    let d: [f64; 3] = std::array::from_fn(|c| ((hi[c] - lo[c]) as f64).max(0.1));
    let area = (d[0] * d[1]).max(d[0] * d[2]).max(d[1] * d[2]);
    (mesh.indices.len() / 3) as f64 / area
}

/// The merge distance for one mesh: `merge_m` scaled by how dense the mesh is against a
/// building-like 2 triangles per m² (square root, between 0.1x and 6x), so tyre stacks, fences
/// and props are simplified first and big plain surfaces least.
fn mesh_merge(mesh: &crate::ac::AcMesh, merge_m: f64) -> f64 {
    if merge_m <= 0.0 {
        return 0.0;
    }
    (merge_m * (density(mesh) / 2.0).sqrt().clamp(0.1, 6.0)).min(MAX_MERGE_M)
}

/// Furthest any vertex may be merged (metres). Road America's budget once took merging to
/// tens of metres, which dragged buildings into spikes; past this, meshes are left out instead.
pub const MAX_MERGE_M: f64 = 1.0;

/// Scenery triangles there would be with `merge_m` (cut-outs counted both ways).
fn count_triangles(scene: &AcScene, opt: &Options, merge_m: f64) -> usize {
    scene
        .meshes
        .iter()
        .enumerate()
        .filter(|(i, m)| is_scenery(scene, m) && !opt.left_out.contains(i))
        .map(|(_, m)| mesh_cost(scene, opt, m, merge_m))
        .sum()
}

/// Triangles one mesh would add (cut-outs counted both ways).
fn mesh_cost(scene: &AcScene, opt: &Options, m: &crate::ac::AcMesh, merge_m: f64) -> usize {
    let cut = opt.two_sided_cutouts && m.material.and_then(|k| scene.materials.get(k)).is_some_and(|mat| mat.blend == 0 && mat.alpha_test);
    merged_triangles(m, mesh_merge(m, merge_m)).0.len() * if cut { 2 } else { 1 }
}

fn plan_cells(scene: &AcScene, track: &Track, offset: [f64; 2], height_base: f64, opt: &Options) -> Plan {
    let ground = NrGround::new(track);
    let mut stats = Stats { cell_m: opt.cell_m, ..Default::default() };

    // materials NR2003 will see: one per (AC material); textures one per (texture, alpha use)
    let mut materials = Vec::new();
    let mut textures: Vec<TextureJob> = Vec::new();
    let mut taken = Vec::new();
    let mut mat_of_ac: Vec<usize> = Vec::with_capacity(scene.materials.len());
    let mut mat_names: Vec<String> = Vec::new();
    for m in &scene.materials {
        // multi-layer materials: their main surface's picture, tiled, instead of the blurry
        // overall colour in txDiffuse (Road America's grass came out as smeared blotches)
        // AC's own detail tiling can't be read from the material (Road America's grass texture
        // coordinates span 1 unit over 1.8 km), so the detail picture is tiled every 6 m by map
        // position
        let (tex, alpha, uv_scale, planar_m) = match m.detail.filter(|&t| t < scene.textures.len()) {
            Some(d) => (Some(d), Alpha::Opaque, 1.0, 6.0),
            None => (m.texture, alpha_of(m), 1.0, 0.0),
        };
        // land with only a colour map: the template's grass, tiled every 8 m
        if m.colour_map_only && opt.ground_texture.is_some() {
            let mut name: String = format!("m_{}", safe_stem(&m.name)).chars().take(28).collect();
            let base = name.clone();
            let mut k = 2;
            while mat_names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                name = format!("{base}{k}");
                k += 1;
            }
            mat_names.push(name.clone());
            mat_of_ac.push(materials.len());
            materials.push(Material { name, mip: Some(GROUND_MIP.into()), alpha: Alpha::Opaque, uv_scale: 1.0, planar_m: 8.0 });
            continue;
        }
        let mip = tex.filter(|&t| t < scene.textures.len()).map(|t| {
            let with_alpha = alpha != Alpha::Opaque;
            match textures.iter().find(|j| j.texture == t && (j.alpha != Alpha::Opaque) == with_alpha) {
                Some(j) => j.mip.clone(),
                None => {
                    let mip = mip_name(&scene.textures[t].name, with_alpha, &mut taken);
                    textures.push(TextureJob { texture: t, source_name: scene.textures[t].name.clone(), mip: mip.clone(), alpha });
                    mip
                }
            }
        });
        let mut name: String = format!("m_{}", safe_stem(&m.name)).chars().take(28).collect();
        let base = name.clone();
        let mut k = 2;
        while mat_names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            name = format!("{base}{k}");
            k += 1;
        }
        mat_names.push(name.clone());
        mat_of_ac.push(materials.len());
        materials.push(Material { name, mip, alpha, uv_scale, planar_m });
    }
    let plain = materials.len();
    materials.push(Material { name: "m_plain".into(), mip: None, alpha: Alpha::Opaque, uv_scale: 1.0, planar_m: 0.0 });

    // triangles into squares: (cell key) -> material -> shapes being filled
    struct Filling {
        shapes: Vec<Shape>,
        // (mesh, vertex, back face) -> index in the last shape
        map: HashMap<(u32, u32, bool), u32>,
    }
    let mut cells: HashMap<(i32, i32), HashMap<(usize, bool), Filling>> = HashMap::new();
    let world = |v: &[f32]| -> [f64; 3] { [v[0] as f64 - offset[0], v[1] as f64 - offset[1], v[2] as f64 - height_base] };
    let dbg = std::env::var_os("TTK_SCENERY_DEBUG").is_some();
    let mut dbg_tally: HashMap<String, (usize, usize, usize, Vec<f64>)> = HashMap::new();
    for (mi, mesh) in scene.meshes.iter().enumerate() {
        if mesh.surface.is_some() {
            stats.meshes_track += 1;
            continue;
        }
        if !mesh.visible {
            stats.meshes_hidden += 1;
            continue;
        }
        let shader = mesh.material.and_then(|m| scene.materials.get(m)).map_or("", |m| m.shader.as_str());
        if shader.eq_ignore_ascii_case("ksGrass") {
            stats.meshes_grass += 1;
            continue;
        }
        if opt.left_out.contains(&mi) {
            continue;
        }
        stats.meshes += 1;
        let small = {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in mesh.verts.chunks_exact(8) {
                for c in 0..3 {
                    lo[c] = lo[c].min(v[c]);
                    hi[c] = hi[c].max(v[c]);
                }
            }
            let d = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
            (d as f64) < opt.small_m
        };
        stats.meshes_small += small as usize;
        let mat = mesh.material.and_then(|m| mat_of_ac.get(m).copied()).unwrap_or(plain);
        let both_ways = opt.two_sided_cutouts && materials[mat].alpha == Alpha::Cutout;
        let uv_k = materials[mat].uv_scale;
        let planar = materials[mat].planar_m;
        let (mtris, moved) = merged_triangles(mesh, mesh_merge(mesh, opt.merge_m));
        let mverts: &[f32] = moved.as_deref().unwrap_or(&mesh.verts);
        let vx = |i: u32| &mverts[i as usize * 8..i as usize * 8 + 8];
        for t in mtris {
            let (a, b, c) = (world(vx(t[0])), world(vx(t[1])), world(vx(t[2])));
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            let area2 = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if area2 < 1e-8 {
                stats.degenerate += 1;
                continue;
            }
            // which way the mesh says it faces (its vertex normals), so the winding can follow
            let vn: [f64; 3] = std::array::from_fn(|k| (vx(t[0])[3 + k] + vx(t[1])[3 + k] + vx(t[2])[3 + k]) as f64);
            let agrees = n[0] * vn[0] + n[1] * vn[1] + n[2] * vn[2] >= 0.0;
            let up = if agrees { n[2] / area2 } else { -n[2] / area2 };
            let cen = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0];
            if dbg && (mesh.name.starts_with("VIS") || mesh.name.starts_with("groove")) {
                let e = dbg_tally.entry(mesh.name.clone()).or_insert((0, 0, 0, Vec::new()));
                if up <= 0.5 {
                    e.0 += 1;
                } else if let Some(h) = ground.height(cen[0], cen[1]) {
                    e.3.push(cen[2] - h);
                } else {
                    e.1 += 1;
                }
                e.2 += 1;
            }
            if up > 0.5 {
                if let Some(h) = ground.height(cen[0], cen[1]) {
                    if cen[2] < h + opt.on_ground_m {
                        stats.on_ground += 1;
                        continue;
                    }
                }
            }
            let key = ((cen[0] / opt.cell_m).floor() as i32, (cen[1] / opt.cell_m).floor() as i32);
            let fill = cells
                .entry(key)
                .or_default()
                .entry((mat, small))
                .or_insert_with(|| Filling { shapes: vec![Shape { material: mat, small, ..Default::default() }], map: HashMap::new() });
            for back in [false, true] {
                if back && !both_ways {
                    continue;
                }
                if fill.shapes.last().unwrap().tris.len() + 1 > MAX_SHAPE_FACES || fill.shapes.last().unwrap().verts.len() + 3 > MAX_SHAPE_VERTS {
                    fill.shapes.push(Shape { material: mat, small, ..Default::default() });
                    fill.map.clear();
                }
                let shape = fill.shapes.last_mut().unwrap();
                let mut idx = [0u32; 3];
                for (k, &vi) in t.iter().enumerate() {
                    idx[k] = *fill.map.entry((mi as u32, vi, back)).or_insert_with(|| {
                        let v = vx(vi);
                        let p = world(v);
                        let s = if back { -1.0 } else { 1.0 };
                        // world for now; made relative to the square's centre below
                        let k = uv_k;
                        let (u, w) = if planar > 0.0 { ((p[0] / planar as f64) as f32, (p[1] / planar as f64) as f32) } else { (v[6] * k, 1.0 - v[7] * k) };
                        shape.verts.push([p[0] as f32, p[1] as f32, p[2] as f32, v[3] * s, v[4] * s, v[5] * s, u, w]);
                        (shape.verts.len() - 1) as u32
                    });
                }
                // counter-clockwise facing the way the normals point (PAS's front)
                let front = agrees != back;
                shape.tris.push(if front { idx } else { [idx[0], idx[2], idx[1]] });
                stats.triangles += 1;
            }
        }
    }

    if dbg {
        let mut v: Vec<_> = dbg_tally.into_iter().collect();
        v.sort_by(|a, b| b.1 .2.cmp(&a.1 .2));
        for (name, (not_up, no_ground, total, mut dz)) in v.into_iter().take(8) {
            dz.sort_by(|a, b| a.total_cmp(b));
            let q = |f: f64| dz.get(((dz.len() as f64 - 1.0) * f) as usize).copied().unwrap_or(f64::NAN);
            eprintln!("[ground] {name}: {total} triangles, {not_up} not facing up, {no_ground} with no NR ground under, height above NR ground 10/50/90%: {:.2} / {:.2} / {:.2} m", q(0.1), q(0.5), q(0.9));
        }
    }
    // squares: centred, named by their place on the map
    let mut keys: Vec<(i32, i32)> = cells.keys().copied().collect();
    keys.sort_unstable();
    let (i0, j0) = keys.iter().fold((i32::MAX, i32::MAX), |a, k| (a.0.min(k.0), a.1.min(k.1)));
    let mut out = Vec::new();
    for key in keys {
        let mut by_mat: Vec<((usize, bool), Filling)> = cells.remove(&key).unwrap().into_iter().collect();
        by_mat.sort_by_key(|(m, _)| (m.1, m.0));
        let mut shapes: Vec<Shape> = by_mat.into_iter().flat_map(|(_, f)| f.shapes).filter(|s| !s.tris.is_empty()).collect();
        let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for s in &shapes {
            for v in &s.verts {
                for c in 0..3 {
                    lo[c] = lo[c].min(v[c] as f64);
                    hi[c] = hi[c].max(v[c] as f64);
                }
            }
        }
        let centre = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1]), lo[2]];
        for s in &mut shapes {
            for v in &mut s.verts {
                for c in 0..3 {
                    v[c] = (v[c] as f64 - centre[c]) as f32;
                }
            }
        }
        out.push(Cell { name: format!("sc_{}_{}", key.0 - i0, key.1 - j0), centre, shapes });
    }
    Plan { cells: out, materials, textures, stats }
}

/// A number as PAS wants it: up to `dp` decimals, no trailing zeros.
fn num(out: &mut String, x: f32, dp: usize) {
    let start = out.len();
    let _ = write!(out, "{:.*}", dp, x);
    if out[start..].contains('.') {
        while out.ends_with('0') {
            out.pop();
        }
        if out.ends_with('.') {
            out.pop();
        }
    }
    if &out[start..] == "-0" {
        out.truncate(start);
        out.push('0');
    }
}

fn v3(out: &mut String, v: &[f32], dp: usize) {
    for (k, x) in v.iter().take(3).enumerate() {
        if k > 0 {
            out.push(' ');
        }
        num(out, *x, dp);
    }
}

/// Shape name inside a square's PAS file.
fn shape_name(k: usize) -> String {
    format!("s{k}")
}

/// The square as a Papyrus Ascii Scene (grammar: blender\nr2003_pas\pas_writer.py).
pub fn pas(cell: &Cell, materials: &[Material]) -> String {
    let mut o = String::with_capacity(cell.triangles() * 300 + 1024);
    o.push_str("# Papyrus Ascii Scene written by the NR2003 Track Toolkit (from Assetto Corsa)\nVERSION\t200\nUNIT_CONVERSION\t1\n");
    for (k, s) in cell.shapes.iter().enumerate() {
        let m = &materials[s.material];
        let _ = write!(
            o,
            "SHAPE {{\n\tNAME \"{}\"\n\tTRANSFORM {{\n\t\tXFORM_ROW0 1 0 0\n\t\tXFORM_ROW1 0 1 0\n\t\tXFORM_ROW2 0 0 1\n\t\tXFORM_ROW3 0 0 0\n\t\tXFORM_POS 0 0 0\n\t}}\n\tMESH {{\n\t\tNUM_VERTICES {}\n\t\tVERTEX_LIST {{\n",
            shape_name(k),
            s.verts.len()
        );
        for (i, v) in s.verts.iter().enumerate() {
            let _ = write!(o, "\t\t\tVERTEX {i}:\t");
            v3(&mut o, &v[0..3], 4);
            o.push('\n');
        }
        let _ = write!(o, "\t\t}}\n\t\tNUM_FACES {}\n\t\tFACE_LIST {{\n", s.tris.len());
        for (i, t) in s.tris.iter().enumerate() {
            let _ = writeln!(o, "\t\t\tFACE {i}: {} {} {}", t[0], t[1], t[2]);
        }
        o.push_str("\t\t}\n\t\tNORMAL_LIST {\n");
        for (i, t) in s.tris.iter().enumerate() {
            let p: [[f32; 3]; 3] = std::array::from_fn(|c| [s.verts[t[c] as usize][0], s.verts[t[c] as usize][1], s.verts[t[c] as usize][2]]);
            let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
            let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
            let mut n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
            n.iter_mut().for_each(|x| *x /= l);
            let _ = write!(o, "\t\t\tFACE_NORMAL {i}: 0 ");
            v3(&mut o, &n, 4);
            o.push('\n');
            for &vi in t {
                let v = &s.verts[vi as usize];
                let mut vn = [v[3], v[4], v[5]];
                let l = (vn[0] * vn[0] + vn[1] * vn[1] + vn[2] * vn[2]).sqrt();
                if l < 1e-6 {
                    vn = n;
                } else {
                    vn.iter_mut().for_each(|x| *x /= l);
                }
                let _ = write!(o, "\t\t\t\tVERTEX_NORMAL {vi}\t");
                v3(&mut o, &vn, 4);
                o.push('\n');
            }
        }
        o.push_str("\t\t}\n\t\tNUM_MATERIAL_REFS 1\n\t\tMATERIAL_REF {\n\t\t\tMATERIAL {\n");
        let _ = write!(
            o,
            "\t\t\t\tNAME \"{}\"\n\t\t\t\tMATERIAL_AMBIENT 0.8 0.8 0.8\n\t\t\t\tMATERIAL_DIFFUSE 0.8 0.8 0.8\n\t\t\t\tMATERIAL_SPECULAR 0 0 0\n\t\t\t\tMATERIAL_SHININESS 0\n\t\t\t\tMATERIAL_REFLECTIVITY 0\n\t\t\t\tMATERIAL_OPACITY 1\n",
            m.name
        );
        if let Some(mip) = &m.mip {
            let _ = writeln!(o, "\t\t\t\tBASE_TEXTURE \"{mip}\"");
        }
        let _ = write!(o, "\t\t\t}}\n\t\t\tNUM_FACES {}\n", s.tris.len());
        for (i, t) in s.tris.iter().enumerate() {
            if m.mip.is_some() {
                let _ = write!(o, "\t\t\tTEXTURE_FACE  {i} BASE_TEXTURE");
                for &vi in t {
                    let v = &s.verts[vi as usize];
                    o.push_str(" (");
                    num(&mut o, v[6], 5);
                    o.push_str(", ");
                    num(&mut o, v[7], 5);
                    o.push(')');
                }
                o.push('\n');
            } else {
                let _ = writeln!(o, "\t\t\tMAT_FACE  {i}");
            }
        }
        o.push_str("\t\t}\n\t}\n}\n");
    }
    o
}

/// make3do script for a square: its big things gone beyond `lod_m`, its small things beyond
/// `detail_lod_m`.
pub fn psg(cell: &Cell, lod_m: f64, detail_lod_m: f64) -> String {
    let mut o = format!("# make3do script written by the NR2003 Track Toolkit (from Assetto Corsa)\ng: MESH_GROUP \"{}.pas\" USEPIVOTS\n", cell.name);
    let (mut big, mut small) = (Vec::new(), Vec::new());
    for (k, s) in cell.shapes.iter().enumerate() {
        let n = shape_name(k);
        let _ = writeln!(o, "m{n}: MESH g {n}");
        if s.small {
            small.push(format!("m{n}"));
        } else {
            big.push(format!("m{n}"));
        }
    }
    let _ = writeln!(o, "none: EMPTY_MESH");
    let mut parts = Vec::new();
    for (name, list, dist) in [("big", &big, lod_m), ("small", &small, detail_lod_m)] {
        if list.is_empty() {
            continue;
        }
        let _ = writeln!(o, "{name}: GROUP ({})", list.join(", "));
        let _ = writeln!(o, "{name}_lod: LOD (0 {name}, {} none)", dist.round());
        parts.push(format!("{name}_lod"));
    }
    let top = if parts.len() == 1 {
        parts[0].clone()
    } else {
        let _ = writeln!(o, "all: GROUP ({})", parts.join(", "));
        "all".to_string()
    };
    let _ = writeln!(o, "OUTPUT {top} \"{}.3do\"", cell.name);
    o
}

/// A texture written by `acdump scene --tex-dir` (index.rgba or index.enc).
pub fn load_texture(dir: &Path, index: usize) -> Result<RgbaImage> {
    let raw = dir.join(format!("{index}.rgba"));
    if raw.is_file() {
        let b = std::fs::read(&raw)?;
        if b.len() < 8 {
            bail!("{} is cut short", raw.display());
        }
        let (w, h) = (u32::from_le_bytes(b[0..4].try_into().unwrap()), u32::from_le_bytes(b[4..8].try_into().unwrap()));
        return RgbaImage::from_raw(w, h, b[8..].to_vec()).with_context(|| format!("{} has the wrong size", raw.display()));
    }
    let enc = dir.join(format!("{index}.enc"));
    let b = std::fs::read(&enc).with_context(|| format!("texture {index} wasn't exported"))?;
    Ok(image::load_from_memory(&b).context("not a picture the toolkit can read")?.to_rgba8())
}

/// Texture size NR2003 gets: powers of two, the longer side at most `max_side`.
pub fn mip_size(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    let p2 = |x: u32| crate::config::Resize::Nearest.apply(x.max(1)).unwrap_or(x).max(1);
    let (mut tw, mut th) = (p2(w), p2(h));
    while tw.max(th) > max_side.max(1) {
        tw = (tw / 2).max(1);
        th = (th / 2).max(1);
    }
    (tw, th)
}

#[derive(Debug, Default)]
pub struct Built {
    /// .3do files made, with the squares they came from.
    pub objects: Vec<(String, [f64; 3])>,
    pub mips: usize,
    /// What went wrong (each a plain sentence); the rest was still made.
    pub problems: Vec<String>,
}

/// Make the .mip and .3do files in `track_dir` (with makemip and make3do from `sandbox_dir`).
/// `tex_dir` holds the textures `acdump scene --tex-dir` wrote. Existing files are replaced.
pub fn build(
    plan: &Plan,
    tex_dir: &Path,
    track_dir: &Path,
    sandbox_dir: &Path,
    opt: &Options,
    progress: &(dyn Fn(&str) + Sync),
) -> Result<Built> {
    for tool in ["make3do.exe", "makemip.exe"] {
        if !sandbox_dir.join(tool).is_file() {
            bail!("{tool} wasn't found in {} (set the Sandbox folder in Settings)", sandbox_dir.display());
        }
    }
    std::fs::create_dir_all(track_dir).with_context(|| format!("creating {}", track_dir.display()))?;
    // objects from an earlier run (named sc_<x>_<y>.3do by this converter) would otherwise be
    // packed into the .dat unused when the squares come out differently
    for e in std::fs::read_dir(track_dir).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().to_ascii_lowercase();
        let ours = n.strip_prefix("sc_").and_then(|r| r.strip_suffix(".3do")).is_some_and(|r| {
            let mut parts = r.split('_');
            let ok = |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
            // sc_<x>_<y>, and parts of split squares: sc_<x>_<y>_<q>...
            let n = parts.clone().count();
            n >= 2 && parts.all(|p| ok(Some(p)))
        });
        if ours && e.path().is_file() {
            let _ = std::fs::remove_file(e.path());
        }
    }
    // the template's grass, for land that only had a colour map
    if let Some((dir, file)) = &opt.ground_texture {
        let base = file.rsplit(['\\', '/']).next().unwrap_or(file);
        if let Some(bytes) = crate::acsupport::track_file(dir, file).or_else(|| crate::acsupport::track_file(&dir.join("trackmat"), base)).or_else(|| crate::acsupport::track_file(dir, base)) {
            std::fs::write(track_dir.join(GROUND_MIP), bytes)?;
        }
    }
    let work = track_dir.join("_ttk_scenery");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).with_context(|| format!("creating {}", work.display()))?;
    let result = Mutex::new(Built::default());
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let total = plan.textures.len() + plan.cells.len();
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 6);
    std::thread::scope(|sc| {
        for w in 0..threads {
            let (result, next, done, work) = (&result, &next, &done, &work);
            sc.spawn(move || {
                let mine = work.join(format!("w{w}"));
                let _ = std::fs::create_dir_all(&mine);
                loop {
                    let k = next.fetch_add(1, Ordering::SeqCst);
                    if k >= total {
                        break;
                    }
                    let outcome = if k < plan.textures.len() {
                        make_mip(&plan.textures[k], tex_dir, track_dir, sandbox_dir, opt, &mine).map(|_| None)
                    } else {
                        let cell = &plan.cells[k - plan.textures.len()];
                        make_3do(cell, &plan.materials, track_dir, sandbox_dir, opt, &work.join(&cell.name)).map(Some)
                    };
                    let mut r = result.lock().unwrap();
                    match outcome {
                        Ok(Some(obj)) => r.objects.push(obj),
                        Ok(None) => r.mips += 1,
                        Err(e) => r.problems.push(format!("{e:#}")),
                    }
                    drop(r);
                    let d = done.fetch_add(1, Ordering::SeqCst) + 1;
                    progress(&format!("scenery: {d} of {total} files made"));
                }
            });
        }
    });
    if !opt.keep_pas {
        let _ = std::fs::remove_dir_all(&work);
    } else {
        for w in 0..threads {
            let _ = std::fs::remove_dir_all(work.join(format!("w{w}")));
        }
    }
    let mut r = result.into_inner().unwrap();
    r.objects.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(r)
}

fn make_mip(job: &TextureJob, tex_dir: &Path, track_dir: &Path, sandbox_dir: &Path, opt: &Options, work: &Path) -> Result<()> {
    let mut img = load_texture(tex_dir, job.texture).with_context(|| format!("texture {}", job.source_name))?;
    match job.alpha {
        Alpha::Opaque => img.pixels_mut().for_each(|p| p.0[3] = 255),
        Alpha::Cutout => img.pixels_mut().for_each(|p| p.0[3] = if p.0[3] >= 128 { 255 } else { 0 }),
        Alpha::Blend => {}
    }
    let size = mip_size(img.width(), img.height(), opt.tex_side);
    let recipe = Recipe { family: Family::Compressed, mapping: 0, priority: 0.1, single_level: false, size: Some(size), cutout_type: false, original_kind: None };
    let target = track_dir.join(&job.mip);
    if target.is_file() {
        std::fs::remove_file(&target).with_context(|| format!("replacing {}", target.display()))?;
    }
    build_image(sandbox_dir, img, &target, &recipe, work).with_context(|| format!("texture {}", job.source_name))?;
    Ok(())
}

fn make_3do(cell: &Cell, materials: &[Material], track_dir: &Path, sandbox_dir: &Path, opt: &Options, work: &Path) -> Result<(String, [f64; 3])> {
    std::fs::create_dir_all(work)?;
    std::fs::write(work.join(format!("{}.pas", cell.name)), pas(cell, materials))?;
    let script = format!("{}.psg", cell.name);
    std::fs::write(work.join(&script), psg(cell, opt.lod_m, opt.detail_lod_m))?;
    let file = format!("{}.3do", cell.name);
    let quiet = Logger::new(|_, _| {});
    let mut out = run_tool(&quiet, &sandbox_dir.join("make3do.exe"), &["-s", script.as_str()], work)?;
    let made = work.join(&file);
    if !out.ok() || !made.is_file() {
        // make3do's limit is on its triangle strips, which loose or merged meshes use up fast:
        // try again with the shapes cut much smaller
        let small = Cell { name: cell.name.clone(), centre: cell.centre, shapes: cell.shapes.iter().flat_map(|sh| cut_shape(sh, 3000)).collect() };
        std::fs::write(work.join(format!("{}.pas", cell.name)), pas(&small, materials))?;
        std::fs::write(work.join(&script), psg(&small, opt.lod_m, opt.detail_lod_m))?;
        let _ = std::fs::remove_file(&made);
        out = run_tool(&quiet, &sandbox_dir.join("make3do.exe"), &["-s", script.as_str()], work)?;
    }
    if !out.ok() || !made.is_file() {
        bail!("make3do couldn't build {file}: {}", out.last_line());
    }
    let target = track_dir.join(&file);
    let _ = std::fs::remove_file(&target);
    std::fs::rename(&made, &target).or_else(|_| std::fs::copy(&made, &target).map(|_| ())).with_context(|| format!("moving {file}"))?;
    if !opt.keep_pas {
        let _ = std::fs::remove_dir_all(work);
    }
    Ok((file, cell.centre))
}

/// A shape cut into shapes of at most `max_verts` vertices.
fn cut_shape(sh: &Shape, max_verts: usize) -> Vec<Shape> {
    let mut out = vec![Shape { material: sh.material, small: sh.small, ..Default::default() }];
    let mut map: HashMap<u32, u32> = HashMap::new();
    for t in &sh.tris {
        if out.last().unwrap().verts.len() + 3 > max_verts {
            out.push(Shape { material: sh.material, small: sh.small, ..Default::default() });
            map.clear();
        }
        let cur = out.last_mut().unwrap();
        let mut idx = [0u32; 3];
        for (k, &vi) in t.iter().enumerate() {
            idx[k] = *map.entry(vi).or_insert_with(|| {
                cur.verts.push(sh.verts[vi as usize]);
                (cur.verts.len() - 1) as u32
            });
        }
        cur.tris.push(idx);
    }
    out
}

/// Place the built objects in the track (once each, at their square's centre, facing 0).
/// Placements of the same files from an earlier run are replaced.
pub fn place(graph: &mut Graph, built: &Built) -> Result<usize> {
    let ours = |f: &str| {
        let f = f.to_ascii_lowercase();
        f.starts_with("sc_") && f.ends_with(".3do")
    };
    let old: Vec<i32> = objects::tsoref_ids(graph)
        .into_iter()
        .filter(|&id| {
            graph
                .nodes
                .get(&id)
                .and_then(|n| n.r("tso"))
                .and_then(|t| graph.nodes.get(&t))
                .and_then(|t| t.s("file"))
                .is_some_and(ours)
        })
        .collect();
    if !old.is_empty() {
        let edits = objects::delete(graph, &old)?;
        objects::apply_nodes(graph, &edits, true);
    }
    let objs: Vec<NewObject> =
        built.objects.iter().map(|(f, c)| NewObject { file: f.clone(), x: c[0], y: c[1], z: c[2], heading: 0.0 }).collect();
    if objs.is_empty() {
        return Ok(0);
    }
    let (edits, ids) = objects::add(graph, &objs)?;
    objects::apply_nodes(graph, &edits, true);
    Ok(ids.len())
}

/// Default work-free folder for scenery textures exported from AC.
pub fn texture_dir_for(out: &Path) -> PathBuf {
    out.join("_ttk_actex")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(mip_size(4096, 2048, 512), (512, 256));
        assert_eq!(mip_size(1000, 1000, 512), (512, 512));
        assert_eq!(mip_size(100, 30, 512), (128, 32));
    }

    #[test]
    fn numbers() {
        let mut s = String::new();
        num(&mut s, 1.5000, 4);
        s.push(' ');
        num(&mut s, -0.00001, 4);
        s.push(' ');
        num(&mut s, 12.0, 4);
        assert_eq!(s, "1.5 0 12");
    }

    #[test]
    fn mip_names() {
        let mut taken = Vec::new();
        assert_eq!(mip_name("Trees Big (2).dds", false, &mut taken), "ac_trees_big__2_.mip");
        assert_eq!(mip_name("trees big (2).png", false, &mut taken), "ac_trees_big__2_2.mip");
        assert_eq!(mip_name("a_very_long_texture_name_indeed.dds", true, &mut taken), "ac_a_very_long_texture__a.mip");
    }
}
