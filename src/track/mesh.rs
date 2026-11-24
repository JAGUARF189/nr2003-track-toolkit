//! Triangles for the track viewer: ground strips and walls ([`build`]), and placed objects as
//! markers ([`objects`], a separate layer so selecting or moving objects doesn't rebuild the
//! track).
//!
//! One interleaved vertex buffer, `VERTEX_FLOATS` per vertex: position (3, metres from
//! `origin`, z up), colour (3), normal (3), segment index (1, -1 = none), texture
//! coordinates (2). Triangles are grouped into [`Batch`]es by texture so each draws with its
//! picture. The colour is used when textures are off or a texture can't be found.

use super::model::{Segment, TexRef, Track};
use std::collections::HashMap;

pub const VERTEX_FLOATS: usize = 12;

/// Triangles drawn with one texture.
#[derive(Clone, Debug)]
pub struct Batch {
    /// Texture file as the track names it (e.g. `trackmat\asphalt.mip`); None = colour only.
    pub texture: Option<String>,
    /// Draw see-through where the picture is (fences, rails).
    pub alpha_test: bool,
    /// First vertex and number of vertices.
    pub first: i32,
    pub count: i32,
}

pub struct TrackMesh {
    pub verts: Vec<f32>,
    /// Triangle corners as vertex numbers; empty = every 3 vertices make a triangle. When set,
    /// the batches' `first`/`count` count indices instead of vertices.
    pub indices: Vec<u32>,
    pub batches: Vec<Batch>,
    /// World position subtracted from every vertex (keeps f32 precise on big tracks).
    pub origin: [f64; 3],
    /// Centerline points per segment, world x/y, for picking and labels.
    pub centerlines: Vec<Vec<[f64; 2]>>,
    /// Size of the track in metres: (width, depth, lowest z, highest z).
    pub extent: [f64; 4],
}

impl TrackMesh {
    /// The texture files the batches use (each once).
    pub fn textures(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for b in &self.batches {
            if let Some(t) = &b.texture {
                if !v.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                    v.push(t.clone());
                }
            }
        }
        v
    }
}

pub fn surface_color(s: i64) -> [f32; 3] {
    match s {
        1 => [0.25, 0.25, 0.27],
        2 => [0.62, 0.62, 0.60],
        3 => [0.93, 0.93, 0.88],
        4 => [0.30, 0.52, 0.22],
        5 => [0.47, 0.34, 0.22],
        6 => [0.82, 0.74, 0.52],
        7 => [0.56, 0.53, 0.47],
        8 => [0.20, 0.40, 0.72],
        12 => [0.82, 0.24, 0.22],
        _ => [0.55, 0.42, 0.62],
    }
}

pub fn wall_color(t: i64) -> [f32; 3] {
    match t {
        2049 | 2050 | 2056 => [0.86, 0.86, 0.83],
        2051..=2053 => [0.22, 0.45, 0.88],
        2054 => [0.18, 0.40, 0.14],
        2055 => [0.10, 0.10, 0.10],
        2057 | 2059 => [0.72, 0.74, 0.78],
        2058 => [0.33, 0.33, 0.36],
        2060 => [0.85, 0.40, 0.10],
        _ => [0.85, 0.62, 0.30],
    }
}

pub const OBJECT_COLOR: [f32; 3] = [1.0, 0.78, 0.15];
pub const OBJECT_SELECTED: [f32; 3] = [1.0, 0.42, 0.10];
pub const OBJECT_LOCKED: [f32; 3] = [0.55, 0.55, 0.58];

/// Batch key: (lower-case texture file or "" for none, alpha test).
type Key = (String, bool);

struct Builder {
    origin: [f64; 3],
    groups: HashMap<Key, (Option<String>, Vec<f32>)>,
    order: Vec<Key>,
}

/// A corner: world position and texture coordinates.
type V = ([f64; 3], [f64; 2]);

impl Builder {
    fn new(origin: [f64; 3]) -> Self {
        Self { origin, groups: HashMap::new(), order: Vec::new() }
    }

    #[allow(clippy::too_many_arguments)]
    fn tri(&mut self, tex: Option<&TexRef>, alpha: bool, a: V, b: V, c: V, col: [f32; 3], seg: f32) {
        let (pa, pb, pc) = (a.0, b.0, c.0);
        let u = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let v = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let mut n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len < 1e-12 {
            return;
        }
        if n[2] < 0.0 {
            // light both sides the same: face the normal upward/outward consistently
            n = [-n[0], -n[1], -n[2]];
        }
        let key: Key = (tex.map(|t| t.file.to_ascii_lowercase()).unwrap_or_default(), alpha && tex.is_some());
        if !self.groups.contains_key(&key) {
            self.order.push(key.clone());
            self.groups.insert(key.clone(), (tex.map(|t| t.file.clone()), Vec::new()));
        }
        let out = &mut self.groups.get_mut(&key).unwrap().1;
        for (p, uv) in [a, b, c] {
            out.extend_from_slice(&[
                (p[0] - self.origin[0]) as f32,
                (p[1] - self.origin[1]) as f32,
                (p[2] - self.origin[2]) as f32,
                col[0],
                col[1],
                col[2],
                (n[0] / len) as f32,
                (n[1] / len) as f32,
                (n[2] / len) as f32,
                seg,
                uv[0] as f32,
                uv[1] as f32,
            ]);
        }
    }

    /// Quad a-b-c-d (in order around it).
    #[allow(clippy::too_many_arguments)]
    fn quad(&mut self, tex: Option<&TexRef>, alpha: bool, a: V, b: V, c: V, d: V, col: [f32; 3], seg: f32) {
        self.tri(tex, alpha, a, b, c, col, seg);
        self.tri(tex, alpha, a, c, d, col, seg);
    }

    fn finish(self) -> (Vec<f32>, Vec<Batch>) {
        let mut verts = Vec::new();
        let mut batches = Vec::new();
        let mut groups = self.groups;
        for key in self.order {
            let (texture, v) = groups.remove(&key).unwrap();
            let first = (verts.len() / VERTEX_FLOATS) as i32;
            let count = (v.len() / VERTEX_FLOATS) as i32;
            verts.extend(v);
            batches.push(Batch { texture, alpha_test: key.1, first, count });
        }
        (verts, batches)
    }
}

fn samples(s: &Segment) -> usize {
    let by_len = s.length / 6.0;
    let by_turn = s.angle.abs().to_degrees() / 2.0;
    (by_len.max(by_turn).ceil() as usize).clamp(1, 96)
}

fn at(s: &Segment, t: f64, dlat: f64, z: f64) -> [f64; 3] {
    let p = s.point(t, dlat);
    [p[0], p[1], z]
}

fn uv(tex: Option<&TexRef>, a: f64, b: f64) -> [f64; 2] {
    tex.map_or([0.0, 0.0], |t| t.at(a, b))
}

pub fn build(track: &Track, show_walls: bool) -> TrackMesh {
    let b = track.bounds();
    let mut zmin = f64::MAX;
    let mut zmax = f64::MIN;
    for s in &track.segments {
        for x in &s.xs {
            for a in x.alt {
                zmin = zmin.min(a);
                zmax = zmax.max(a);
            }
        }
    }
    if zmin > zmax {
        zmin = 0.0;
        zmax = 0.0;
    }
    let origin = [0.5 * (b[0] + b[2]), 0.5 * (b[1] + b[3]), zmin];
    let mut m = Builder::new(origin);
    let mut centerlines = Vec::with_capacity(track.segments.len());

    for s in &track.segments {
        let seg = s.index as f32;
        let n = samples(s);
        let ts: Vec<f64> = (0..=n).map(|k| k as f64 / n as f64).collect();
        centerlines.push(ts.iter().map(|&t| s.point(t, 0.0)).collect());

        // Ground strips, paired by F-section order at the middle of the segment.
        let mid = 0.5;
        let mut order: Vec<usize> = (0..s.fs.len()).collect();
        order.sort_by(|&a, &b| {
            let da = s.fs[a].dlat[0] + (s.fs[a].dlat[1] - s.fs[a].dlat[0]) * mid;
            let db = s.fs[b].dlat[0] + (s.fs[b].dlat[1] - s.fs[b].dlat[0]) * mid;
            da.total_cmp(&db)
        });
        let fd = |k: usize, t: f64| {
            let f = &s.fs[order[k]];
            f.dlat[0] + (f.dlat[1] - f.dlat[0]) * t
        };
        let profiles: Vec<Vec<(f64, f64)>> = ts.iter().map(|&t| s.profile(t)).collect();
        for k in 0..order.len() {
            let f = &s.fs[order[k]];
            let col = surface_color(f.surface);
            let tex = f.tex.as_ref();
            let left = |t: f64| if k + 1 < order.len() { fd(k + 1, t) } else { s.max_dlat(t) };
            // Lateral break points: X sections inside the strip at mid-segment, as fractions.
            let (m0, m1) = (fd(k, mid), left(mid));
            if m1 - m0 < 1e-3 {
                continue;
            }
            let mut us = vec![0.0];
            for (d, _) in s.profile(mid) {
                let u = (d - m0) / (m1 - m0);
                if u > 0.01 && u < 0.99 {
                    us.push(u);
                }
            }
            us.push(1.0);
            us.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
            for i in 0..n {
                let (t0, t1) = (ts[i], ts[i + 1]);
                // texture corners: a = along (start to end), b = across (right edge to left edge)
                let row = |t: f64, p: &Vec<(f64, f64)>| -> Vec<V> {
                    let (d0, d1) = (fd(k, t), left(t));
                    us.iter()
                        .map(|&u| {
                            let d = d0 + (d1 - d0) * u;
                            (at(s, t, d, super::model::height_in(p, d)), uv(tex, t, u))
                        })
                        .collect()
                };
                let r0 = row(t0, &profiles[i]);
                let r1 = row(t1, &profiles[i + 1]);
                for j in 0..us.len() - 1 {
                    m.quad(tex, false, r0[j], r0[j + 1], r1[j + 1], r1[j], col, seg);
                }
            }
        }

        if show_walls {
            for w in &s.ws {
                let col = wall_color(w.wall_type);
                let lerp = |a: [f64; 2], t: f64| a[0] + (a[1] - a[0]) * t;
                // Corners across the wall at t: bottom-right, top-right, top-left, bottom-left.
                let ring = |t: f64| {
                    let d = lerp(w.dlat, t);
                    let half = 0.5 * lerp(w.width, t).max(0.05);
                    let h = lerp(w.height, t).max(0.05);
                    let g = s.height(t, d);
                    [at(s, t, d - half, g), at(s, t, d - half, g + h), at(s, t, d + half, g + h), at(s, t, d + half, g)]
                };
                let whole = [super::model::WallPart { from: 0.0, to: 1.0, faces: [None, None, None, None, None] }];
                let parts: &[super::model::WallPart] = if w.parts.is_empty() { &whole } else { &w.parts };
                for (pi, part) in parts.iter().enumerate() {
                    let (from, to) = (part.from.min(part.to), part.to.max(part.from));
                    if to - from < 1e-6 {
                        continue;
                    }
                    let np = ((n as f64 * (to - from)).ceil() as usize).max(1);
                    let pts: Vec<f64> = (0..=np).map(|k| from + (to - from) * k as f64 / np as f64).collect();
                    let [right, top, leftf, near, far] = &part.faces;
                    let (right, top, leftf) = (right.as_ref(), top.as_ref(), leftf.as_ref());
                    for i in 0..np {
                        let (ta, tb) = (pts[i], pts[i + 1]);
                        let (a, c) = (ring(ta), ring(tb));
                        let (aa, ab) = ((ta - from) / (to - from), (tb - from) / (to - from));
                        // right side: a along, b bottom (0) to top (1)
                        m.quad(right, true, (a[0], uv(right, aa, 0.0)), (c[0], uv(right, ab, 0.0)), (c[1], uv(right, ab, 1.0)), (a[1], uv(right, aa, 1.0)), col, seg);
                        // top: a along, b right (0) to left (1)
                        m.quad(top, true, (a[1], uv(top, aa, 0.0)), (c[1], uv(top, ab, 0.0)), (c[2], uv(top, ab, 1.0)), (a[2], uv(top, aa, 1.0)), col, seg);
                        // left side: a along, b top (0) to bottom (1)
                        m.quad(leftf, true, (a[2], uv(leftf, aa, 0.0)), (c[2], uv(leftf, ab, 0.0)), (c[3], uv(leftf, ab, 1.0)), (a[3], uv(leftf, aa, 1.0)), col, seg);
                    }
                    // End caps: near end on the first part, far end on the last.
                    if pi == 0 {
                        let r = ring(from);
                        let t = near.as_ref();
                        // a: left to right, b: bottom to top
                        m.quad(t, true, (r[3], uv(t, 0.0, 0.0)), (r[0], uv(t, 1.0, 0.0)), (r[1], uv(t, 1.0, 1.0)), (r[2], uv(t, 0.0, 1.0)), col, seg);
                    }
                    if pi + 1 == parts.len() {
                        let r = ring(to);
                        let t = far.as_ref();
                        // a: right to left, b: bottom to top
                        m.quad(t, true, (r[0], uv(t, 0.0, 0.0)), (r[3], uv(t, 1.0, 0.0)), (r[2], uv(t, 1.0, 1.0)), (r[1], uv(t, 0.0, 1.0)), col, seg);
                    }
                }
            }
        }
    }

    let (verts, batches) = m.finish();
    TrackMesh { verts, indices: Vec::new(), batches, origin, centerlines, extent: [b[2] - b[0], b[3] - b[1], zmin, zmax] }
}

/// Markers for the placed objects (2 m boxes, 3 m tall), coloured by `color(index)`, in the
/// same coordinates as the track mesh made with `origin`. One batch, no texture.
pub fn objects(track: &Track, origin: [f64; 3], color: impl Fn(usize) -> [f32; 3]) -> Vec<f32> {
    let mut m = Builder::new(origin);
    for (k, o) in track.objects.iter().enumerate() {
        let col = color(k);
        let (h, half) = (3.0, 1.0);
        let (c, sn) = (o.heading.cos(), o.heading.sin());
        let corner = |dx: f64, dy: f64, z: f64| ([o.x + dx * c - dy * sn, o.y + dx * sn + dy * c, z], [0.0, 0.0]);
        let z0 = o.z;
        let z1 = o.z + h;
        let bot = [corner(-half, -half, z0), corner(half, -half, z0), corner(half, half, z0), corner(-half, half, z0)];
        let top = [corner(-half, -half, z1), corner(half, -half, z1), corner(half, half, z1), corner(-half, half, z1)];
        m.quad(None, false, top[0], top[1], top[2], top[3], col, -1.0);
        for k in 0..4 {
            let j = (k + 1) % 4;
            m.quad(None, false, bot[k], bot[j], top[j], top[k], col, -1.0);
        }
    }
    m.finish().0
}
