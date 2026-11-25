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

/// Most faces in one shape (make3do fails at 32768).
pub const MAX_SHAPE_FACES: usize = 30_000;
/// Most vertices in one shape (make3do fails at 16538).
pub const MAX_SHAPE_VERTS: usize = 16_000;

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
    pub on_ground_m: f64,
    /// Keep the .pas/.psg files make3do was given (for editing in Blender).
    pub keep_pas: bool,
    /// Most object files (squares). Installed tracks use at most 299 different object files
    /// (Charlotte_2018_BBMC_Day; Las Vegas 255), so the squares grow until there are at most
    /// this many.
    pub max_objects: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { cell_m: 150.0, lod_m: 800.0, detail_lod_m: 300.0, small_m: 8.0, tex_side: 512, two_sided_cutouts: true, on_ground_m: 0.5, keep_pas: false, max_objects: 250 }
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
    loop {
        let p = plan_cells(scene, track, offset, height_base, &o);
        let n = p.cells.len();
        if n <= opt.max_objects.max(1) || o.cell_m >= 3000.0 {
            return p;
        }
        // squares' count goes roughly with 1 / size²
        o.cell_m = (o.cell_m * (n as f64 / opt.max_objects.max(1) as f64).sqrt() * 1.05).min(3000.0);
    }
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
        let alpha = alpha_of(m);
        let mip = m.texture.filter(|&t| t < scene.textures.len()).map(|t| {
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
        materials.push(Material { name, mip, alpha });
    }
    let plain = materials.len();
    materials.push(Material { name: "m_plain".into(), mip: None, alpha: Alpha::Opaque });

    // triangles into squares: (cell key) -> material -> shapes being filled
    struct Filling {
        shapes: Vec<Shape>,
        // (mesh, vertex, back face) -> index in the last shape
        map: HashMap<(u32, u32, bool), u32>,
    }
    let mut cells: HashMap<(i32, i32), HashMap<(usize, bool), Filling>> = HashMap::new();
    let world = |v: &[f32]| -> [f64; 3] { [v[0] as f64 - offset[0], v[1] as f64 - offset[1], v[2] as f64 - height_base] };
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
        let vx = |i: u32| &mesh.verts[i as usize * 8..i as usize * 8 + 8];
        for t in mesh.indices.chunks_exact(3) {
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
                        shape.verts.push([p[0] as f32, p[1] as f32, p[2] as f32, v[3] * s, v[4] * s, v[5] * s, v[6], 1.0 - v[7]]);
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
            ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
        });
        if ours && e.path().is_file() {
            let _ = std::fs::remove_file(e.path());
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
    let out = run_tool(&quiet, &sandbox_dir.join("make3do.exe"), &["-s", script.as_str()], work)?;
    let made = work.join(&file);
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
