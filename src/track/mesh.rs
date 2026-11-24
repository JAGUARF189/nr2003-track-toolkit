//! Triangles for the track viewer: ground strips coloured by surface and walls as boxes
//! ([`build`]), and placed objects as markers ([`objects`], a separate layer so selecting or
//! moving objects doesn't rebuild the track). One interleaved vertex buffer, `VERTEX_FLOATS` per vertex:
//! position (3, metres from `origin`, z up), colour (3), normal (3), segment index (1, -1 = none).

use super::model::{Segment, Track};

pub const VERTEX_FLOATS: usize = 10;

pub struct TrackMesh {
    pub verts: Vec<f32>,
    /// World position subtracted from every vertex (keeps f32 precise on big tracks).
    pub origin: [f64; 3],
    /// Centerline points per segment, world x/y, for picking and labels.
    pub centerlines: Vec<Vec<[f64; 2]>>,
    /// Size of the track in metres: (width, depth, lowest z, highest z).
    pub extent: [f64; 4],
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

struct Builder {
    verts: Vec<f32>,
    origin: [f64; 3],
}

impl Builder {
    fn tri(&mut self, a: [f64; 3], b: [f64; 3], c: [f64; 3], col: [f32; 3], seg: f32) {
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let mut n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len < 1e-12 {
            return;
        }
        if n[2] < 0.0 {
            // light both sides the same: face the normal upward/outward consistently
            n = [-n[0], -n[1], -n[2]];
        }
        for p in [a, b, c] {
            self.verts.extend_from_slice(&[
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
            ]);
        }
    }

    fn quad(&mut self, a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3], col: [f32; 3], seg: f32) {
        self.tri(a, b, c, col, seg);
        self.tri(a, c, d, col, seg);
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
    let mut m = Builder { verts: Vec::new(), origin };
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
            let surface = s.fs[order[k]].surface;
            let col = surface_color(surface);
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
                let row = |t: f64, p: &Vec<(f64, f64)>| -> Vec<[f64; 3]> {
                    let (d0, d1) = (fd(k, t), left(t));
                    us.iter()
                        .map(|u| {
                            let d = d0 + (d1 - d0) * u;
                            at(s, t, d, super::model::height_in(p, d))
                        })
                        .collect()
                };
                let r0 = row(t0, &profiles[i]);
                let r1 = row(t1, &profiles[i + 1]);
                for j in 0..us.len() - 1 {
                    m.quad(r0[j], r0[j + 1], r1[j + 1], r1[j], col, seg);
                }
            }
        }

        if show_walls {
            for w in &s.ws {
                let col = wall_color(w.wall_type);
                let lerp = |a: [f64; 2], t: f64| a[0] + (a[1] - a[0]) * t;
                let ring = |t: f64, p: &Vec<(f64, f64)>| {
                    let d = lerp(w.dlat, t);
                    let half = 0.5 * lerp(w.width, t).max(0.05);
                    let h = lerp(w.height, t).max(0.05);
                    let g = super::model::height_in(p, d);
                    [
                        at(s, t, d - half, g),
                        at(s, t, d - half, g + h),
                        at(s, t, d + half, g + h),
                        at(s, t, d + half, g),
                    ]
                };
                for i in 0..n {
                    let a = ring(ts[i], &profiles[i]);
                    let c = ring(ts[i + 1], &profiles[i + 1]);
                    m.quad(a[0], a[1], c[1], c[0], col, seg); // right face
                    m.quad(a[1], a[2], c[2], c[1], col, seg); // top
                    m.quad(a[2], a[3], c[3], c[2], col, seg); // left face
                }
            }
        }
    }

    TrackMesh { verts: m.verts, origin, centerlines, extent: [b[2] - b[0], b[3] - b[1], zmin, zmax] }
}

/// Markers for the placed objects (2 m boxes, 3 m tall), coloured by `color(index)`, in the
/// same coordinates as the track mesh made with `origin`.
pub fn objects(track: &Track, origin: [f64; 3], color: impl Fn(usize) -> [f32; 3]) -> Vec<f32> {
    let mut m = Builder { verts: Vec::with_capacity(track.objects.len() * 30 * VERTEX_FLOATS), origin };
    for (k, o) in track.objects.iter().enumerate() {
        let col = color(k);
        let (h, half) = (3.0, 1.0);
        let (c, sn) = (o.heading.cos(), o.heading.sin());
        let corner = |dx: f64, dy: f64, z: f64| [o.x + dx * c - dy * sn, o.y + dx * sn + dy * c, z];
        let z0 = o.z;
        let z1 = o.z + h;
        let bot = [corner(-half, -half, z0), corner(half, -half, z0), corner(half, half, z0), corner(-half, half, z0)];
        let top = [corner(-half, -half, z1), corner(half, -half, z1), corner(half, half, z1), corner(-half, half, z1)];
        m.quad(top[0], top[1], top[2], top[3], col, -1.0);
        for k in 0..4 {
            let j = (k + 1) % 4;
            m.quad(bot[k], bot[j], top[j], top[k], col, -1.0);
        }
    }
    m.verts
}
