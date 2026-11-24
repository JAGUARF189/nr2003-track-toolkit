//! Assetto Corsa tracks, read through the C++ `acdump` tool (cpp/tools/acdump.cpp): the track
//! folders, their layouts, and a 3D scene (models, textures, driving surfaces, markers and the
//! AI line) for the viewer. The first step of converting an AC track to NR2003.

use crate::track::mesh::{Batch, TrackMesh, VERTEX_FLOATS};
use anyhow::{bail, Context, Result};
use image::RgbaImage;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Deserialize, Clone, Debug)]
pub struct AcModelInfo {
    pub file: String,
    pub exists: bool,
}

#[derive(Deserialize, Clone, Debug)]
pub struct AcSurfaceInfo {
    pub key: String,
    pub valid: bool,
    pub pitlane: bool,
}

#[derive(Deserialize, Clone, Debug)]
pub struct AcLayout {
    /// "" = the default layout
    pub name: String,
    pub title: String,
    pub models: Vec<AcModelInfo>,
    pub surfaces: Vec<AcSurfaceInfo>,
    pub ai_points: usize,
    pub lap_m: f64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct AcInfo {
    pub name: String,
    pub title: String,
    pub layouts: Vec<AcLayout>,
}

pub fn find_acdump() -> Option<PathBuf> {
    crate::track::graph::find_tool("acdump.exe", "TTK_ACDUMP")
}

fn run(args: &[&std::ffi::OsStr]) -> Result<(Vec<u8>, String)> {
    let tool = find_acdump().context("acdump.exe was not found. It must sit next to track-toolkit.exe (build it from the cpp folder).")?;
    let mut cmd = Command::new(&tool);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().with_context(|| format!("running {}", tool.display()))?;
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        let msg = err.lines().filter(|l| l.starts_with("error:")).collect::<Vec<_>>().join(" ");
        bail!("{}", if msg.is_empty() { err.trim().to_string() } else { msg.trim_start_matches("error: ").to_string() });
    }
    Ok((out.stdout, err))
}

/// Layouts, models, surfaces and AI line of an AC track folder.
pub fn info(dir: &Path) -> Result<AcInfo> {
    let (out, _) = run(&["info".as_ref(), dir.as_os_str()])?;
    serde_json::from_slice(&out).context("reading acdump's answer")
}

/// Write the 3D scene of `layout` ("" = default) to `out`. Returns acdump's summary line.
pub fn export_scene(dir: &Path, layout: &str, out: &Path, tex_side: u32) -> Result<String> {
    let layout = if layout.is_empty() { "-" } else { layout };
    let side = tex_side.to_string();
    let mut args: Vec<&std::ffi::OsStr> = vec!["scene".as_ref(), dir.as_os_str(), layout.as_ref(), out.as_os_str()];
    if tex_side == 0 {
        args.push("--no-tex".as_ref());
    } else {
        args.push("--tex".as_ref());
        args.push(side.as_ref());
    }
    let (_, err) = run(&args)?;
    Ok(err.lines().filter(|l| !l.starts_with("warning")).last().unwrap_or("").to_string())
}

/// The AC `content\tracks` folder: from Steam's library list, else the usual places.
pub fn find_tracks_dir() -> Option<PathBuf> {
    let tail = Path::new("steamapps").join("common").join("assettocorsa").join("content").join("tracks");
    let mut roots: Vec<PathBuf> = Vec::new();
    for steam in [r"C:\Program Files (x86)\Steam", r"C:\Program Files\Steam"] {
        roots.push(PathBuf::from(steam));
        // libraryfolders.vdf lists the other Steam libraries: "path"   "E:\\SteamLibrary"
        if let Ok(text) = std::fs::read_to_string(Path::new(steam).join("steamapps").join("libraryfolders.vdf")) {
            for line in text.lines() {
                let parts: Vec<&str> = line.split('"').collect();
                if parts.len() >= 4 && parts[1].eq_ignore_ascii_case("path") {
                    roots.push(PathBuf::from(parts[3].replace("\\\\", "\\")));
                }
            }
        }
    }
    for d in b'C'..=b'Z' {
        for sub in ["SteamLibrary", "Steam", r"Program Files (x86)\Steam", "Games\\Steam"] {
            roots.push(PathBuf::from(format!("{}:\\{sub}", d as char)));
        }
    }
    roots.into_iter().map(|r| r.join(&tail)).find(|p| p.is_dir())
}

/// Track folders in an AC `content\tracks` folder: (folder name, path), sorted.
pub fn list_tracks(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            let name = crate::util::file_name(p).to_string();
            p.join("models.ini").is_file()
                || p.join(format!("{name}.kn5")).is_file()
                || std::fs::read_dir(p).into_iter().flatten().flatten().any(|e| {
                    let n = e.file_name().to_string_lossy().to_ascii_lowercase();
                    n.starts_with("models_") && n.ends_with(".ini")
                })
        })
        .map(|p| (crate::util::file_name(&p).to_string(), p))
        .collect();
    out.sort_by_key(|t| t.0.to_ascii_lowercase());
    out
}

// ------------------------------------------------------------------ the scene file

pub struct AcTexture {
    pub name: String,
    pub image: Option<RgbaImage>,
}

pub struct AcMaterial {
    pub name: String,
    pub shader: String,
    /// Index into `textures`, or None.
    pub texture: Option<usize>,
    pub alpha_test: bool,
    pub blend: u8,
}

pub struct AcSurface {
    pub key: String,
    pub valid: bool,
    pub pitlane: bool,
}

pub struct AcMesh {
    pub name: String,
    pub model: String,
    pub material: Option<usize>,
    /// Index into `surfaces` if cars drive on it.
    pub surface: Option<usize>,
    pub visible: bool,
    pub transparent: bool,
    /// 8 floats per vertex: position (Track Toolkit axes, z up), normal, uv.
    pub verts: Vec<f32>,
    pub indices: Vec<u32>,
}

pub struct AcMarker {
    pub name: String,
    pub pos: [f32; 3],
    pub forward: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct AcAiPoint {
    pub pos: [f32; 3],
    pub distance: f32,
    pub left: f32,
    pub right: f32,
}

pub struct AcScene {
    pub textures: Vec<AcTexture>,
    pub materials: Vec<AcMaterial>,
    pub surfaces: Vec<AcSurface>,
    pub meshes: Vec<AcMesh>,
    pub markers: Vec<AcMarker>,
    pub ai: Vec<AcAiPoint>,
}

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.p + n > self.b.len() {
            bail!("the scene file is cut short");
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn str(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).to_string())
    }
    fn f32s(&mut self, n: usize) -> Result<Vec<f32>> {
        let raw = self.take(n * 4)?;
        Ok(raw.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect())
    }
    fn count(&mut self, each: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n.saturating_mul(each) > self.b.len() - self.p {
            bail!("the scene file has a bad count");
        }
        Ok(n)
    }
}

impl AcScene {
    /// Read a scene written by `acdump scene`. PNG/JPEG textures are decoded here, at about
    /// `tex_side` pixels.
    pub fn read(path: &Path, tex_side: u32) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let mut c = Cur { b: &bytes, p: 0 };
        if c.take(8)? != b"ACSCENE1" {
            bail!("{} isn't an acdump scene", path.display());
        }
        let mut textures = Vec::new();
        for _ in 0..c.count(12)? {
            let name = c.str()?;
            let (w, h) = (c.u32()?, c.u32()?);
            let image = if w > 0 && h > 0 {
                let px = c.take(w as usize * h as usize * 4)?.to_vec();
                RgbaImage::from_raw(w, h, px)
            } else {
                let n = c.u32()? as usize;
                let enc = c.take(n)?;
                (n > 0)
                    .then(|| image::load_from_memory(enc).ok())
                    .flatten()
                    .map(|im| {
                        let im = im.to_rgba8();
                        let (w, h) = (im.width(), im.height());
                        if w.max(h) > tex_side && tex_side > 0 {
                            let f = tex_side as f64 / w.max(h) as f64;
                            image::imageops::resize(
                                &im,
                                ((w as f64 * f) as u32).max(1),
                                ((h as f64 * f) as u32).max(1),
                                image::imageops::FilterType::Triangle,
                            )
                        } else {
                            im
                        }
                    })
            };
            textures.push(AcTexture { name, image });
        }
        let mut materials = Vec::new();
        for _ in 0..c.count(14)? {
            let name = c.str()?;
            let shader = c.str()?;
            let t = c.i32()?;
            let alpha_test = c.u8()? != 0;
            let blend = c.u8()?;
            materials.push(AcMaterial { name, shader, texture: usize::try_from(t).ok(), alpha_test, blend });
        }
        let mut surfaces = Vec::new();
        for _ in 0..c.count(6)? {
            surfaces.push(AcSurface { key: c.str()?, valid: c.u8()? != 0, pitlane: c.u8()? != 0 });
        }
        let mut meshes = Vec::new();
        for _ in 0..c.count(22)? {
            let name = c.str()?;
            let model = c.str()?;
            let material = usize::try_from(c.i32()?).ok();
            let surface = usize::try_from(c.i32()?).ok();
            let visible = c.u8()? != 0;
            let transparent = c.u8()? != 0;
            let nv = c.count(32)?;
            let verts = c.f32s(nv * 8)?;
            let ni = c.count(4)?;
            let raw = c.take(ni * 4)?;
            let indices: Vec<u32> = raw.chunks_exact(4).map(|q| u32::from_le_bytes(q.try_into().unwrap())).collect();
            if indices.iter().any(|&i| i as usize >= nv) {
                bail!("mesh {name} has a bad triangle");
            }
            meshes.push(AcMesh { name, model, material, surface, visible, transparent, verts, indices });
        }
        let mut markers = Vec::new();
        for _ in 0..c.count(28)? {
            let name = c.str()?;
            let v = c.f32s(6)?;
            markers.push(AcMarker { name, pos: [v[0], v[1], v[2]], forward: [v[3], v[4], v[5]] });
        }
        let mut ai = Vec::new();
        for _ in 0..c.count(24)? {
            let v = c.f32s(6)?;
            ai.push(AcAiPoint { pos: [v[0], v[1], v[2]], distance: v[3], left: v[4], right: v[5] });
        }
        Ok(Self { textures, materials, surfaces, meshes, markers, ai })
    }

    pub fn triangles(&self) -> usize {
        self.meshes.iter().map(|m| m.indices.len() / 3).sum()
    }

    /// Markers whose name starts with `prefix` (e.g. "AC_PIT_", "AC_START_").
    pub fn markers_named<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a AcMarker> + 'a {
        self.markers.iter().filter(move |m| m.name.to_ascii_uppercase().starts_with(prefix))
    }
}

/// Colour for a driving surface, guessed from its AC name.
pub fn surface_colour(key: &str, valid: bool) -> [f32; 3] {
    let k = key.to_ascii_uppercase();
    let has = |w: &[&str]| w.iter().any(|x| k.contains(x));
    if has(&["KERB", "CURB", "RUMBLE"]) {
        [0.85, 0.22, 0.20]
    } else if has(&["WALL", "BARRIER", "FENCE", "TYRE", "TIRE"]) {
        [0.90, 0.90, 0.86]
    } else if has(&["SAND", "GRAVEL", "TRAP"]) {
        [0.84, 0.74, 0.50]
    } else if has(&["GRASS", "GRS", "GREEN", "OFFTRACK", "OUT", "PEN", "DIRT", "TOMB"]) || !valid {
        [0.30, 0.55, 0.22]
    } else if has(&["PIT"]) {
        [0.35, 0.45, 0.65]
    } else if has(&["CONCRETE", "CARPET", "MEDIO"]) {
        [0.62, 0.62, 0.60]
    } else {
        [0.26, 0.26, 0.28] // road
    }
}

/// What the AC view shows.
#[derive(Clone, Copy, PartialEq)]
pub struct ViewOptions {
    /// Buildings, trees, stands... (everything cars don't drive on).
    pub scenery: bool,
    /// Draw driving surfaces in plain colours by type (includes the hidden physics-only ones).
    pub colour_surfaces: bool,
}

/// Triangles for the viewer, indexed, one batch per texture.
pub fn build_mesh(s: &AcScene, opt: ViewOptions) -> TrackMesh {
    let include = |m: &AcMesh| match m.surface {
        Some(_) => m.visible || opt.colour_surfaces,
        None => m.visible && opt.scenery,
    };
    // bounds of what's shown (all meshes if nothing is)
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for m in s.meshes.iter().filter(|m| include(m)) {
        for v in m.verts.chunks_exact(8) {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
    }
    for p in &s.ai {
        for k in 0..3 {
            lo[k] = lo[k].min(p.pos[k]);
            hi[k] = hi[k].max(p.pos[k]);
        }
    }
    if lo[0] > hi[0] {
        lo = [0.0; 3];
        hi = [0.0; 3];
    }
    let origin = [0.5 * (lo[0] + hi[0]) as f64, 0.5 * (lo[1] + hi[1]) as f64, lo[2] as f64];
    let o = [origin[0] as f32, origin[1] as f32, origin[2] as f32];

    let mut verts: Vec<f32> = Vec::new();
    let mut groups: HashMap<(Option<usize>, bool), Vec<u32>> = HashMap::new();
    let mut order: Vec<(Option<usize>, bool)> = Vec::new();
    for m in s.meshes.iter().filter(|m| include(m)) {
        let mat = m.material.and_then(|i| s.materials.get(i));
        let surf = m.surface.and_then(|i| s.surfaces.get(i));
        let plain = opt.colour_surfaces && surf.is_some();
        let tex = if plain { None } else { mat.and_then(|x| x.texture).filter(|&t| s.textures.get(t).is_some_and(|t| t.image.is_some())) };
        let alpha = mat.is_some_and(|x| x.alpha_test || x.blend > 0);
        let col = match surf {
            Some(sf) if plain => surface_colour(&sf.key, sf.valid),
            _ => [0.62, 0.62, 0.60],
        };
        let base = (verts.len() / VERTEX_FLOATS) as u32;
        for v in m.verts.chunks_exact(8) {
            verts.extend_from_slice(&[v[0] - o[0], v[1] - o[1], v[2] - o[2], col[0], col[1], col[2], v[3], v[4], v[5], -1.0, v[6], v[7]]);
        }
        let key = (tex, alpha && tex.is_some());
        let g = groups.entry(key).or_insert_with(|| {
            order.push(key);
            Vec::new()
        });
        g.extend(m.indices.iter().map(|&i| base + i));
    }
    let mut indices = Vec::new();
    let mut batches = Vec::new();
    for key in order {
        let g = groups.remove(&key).unwrap();
        batches.push(Batch {
            texture: key.0.map(|t| s.textures[t].name.clone()),
            alpha_test: key.1,
            first: indices.len() as i32,
            count: g.len() as i32,
        });
        indices.extend(g);
    }
    let line: Vec<[f64; 2]> = s.ai.iter().map(|p| [p.pos[0] as f64, p.pos[1] as f64]).collect();
    TrackMesh {
        verts,
        indices,
        batches,
        origin,
        centerlines: vec![line],
        extent: [(hi[0] - lo[0]) as f64, (hi[1] - lo[1]) as f64, lo[2] as f64, hi[2] as f64],
    }
}
