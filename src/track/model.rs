//! A track as plain numbers: centerline segments with their cross-section, ground strips,
//! walls and placed objects, plus the geometry maths to turn them into positions.
//!
//! Conventions (checked against martinsville = 0.526 mi and hillside = 1.000 mi):
//! - World x/y in metres, z up. Heading in radians, 0 = +x, counter-clockwise positive.
//! - `dlat` is the sideways offset from the centerline in metres, **positive to the left**.
//! - A segment is a straight (`type 0`) or a circular arc (`type 1`) from its start
//!   (x, y, heading) to its end. A positive turn angle is a left turn.
//! - X sections give the ground height at a dlat at the segment start and end; `slope` is the
//!   grade along the track there (rise per metre), so height runs as a Hermite curve along the
//!   segment and linearly across between X sections.
//! - Each F section starts a ground strip that runs left to the next F section.

use super::graph::{Graph, Node};
use anyhow::{bail, Result};
use std::f64::consts::PI;

#[derive(Clone, Debug)]
pub struct XSec {
    pub dlat: [f64; 2],
    pub alt: [f64; 2],
    pub slope: [f64; 2],
}

#[derive(Clone, Debug)]
pub struct FSec {
    pub dlat: [f64; 2],
    pub surface: i64,
}

#[derive(Clone, Debug)]
pub struct WSec {
    pub dlat: [f64; 2],
    pub height: [f64; 2],
    pub width: [f64; 2],
    pub wall_type: i64,
}

#[derive(Clone, Debug)]
pub struct Segment {
    pub index: usize,
    pub curve: bool,
    pub start: [f64; 3], // x, y, heading
    pub end: [f64; 3],
    /// Signed turn in radians (positive = left). 0 for straights.
    pub angle: f64,
    /// Centerline radius in metres (0 for straights).
    pub radius: f64,
    /// Centerline length in metres.
    pub length: f64,
    pub xs: Vec<XSec>,
    pub fs: Vec<FSec>,
    pub ws: Vec<WSec>,
}

#[derive(Clone, Debug)]
pub struct Placed {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub heading: f64,
    pub file: String,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub night: bool,
    pub segments: Vec<Segment>,
    pub objects: Vec<Placed>,
    pub length: f64,
}

fn wrap_pi(a: f64) -> f64 {
    let mut a = (a + PI) % (2.0 * PI);
    if a < 0.0 {
        a += 2.0 * PI;
    }
    a - PI
}

fn pair(n: &Node, a: &str, b: &str) -> [f64; 2] {
    [n.f(a), n.f(b)]
}

impl Track {
    pub fn from_graph(g: &Graph) -> Result<Self> {
        let root = g.root();
        if root.class != "TrackDescriptor" {
            bail!("this file isn't a track (it starts with {})", root.class);
        }
        let mut segments = Vec::new();
        for (index, s) in g.list(root, "seg", root.i("segment_count")).into_iter().enumerate() {
            let start = [s.f("sx"), s.f("sy"), s.f("sh")];
            let end = [s.f("ex"), s.f("ey"), s.f("eh")];
            let chord = (end[0] - start[0]).hypot(end[1] - start[1]);
            let curve = s.i("type") != 0;
            let (angle, radius, length) = if curve {
                let a = wrap_pi(end[2] - start[2]);
                if a.abs() < 1e-9 {
                    (0.0, 0.0, chord)
                } else {
                    let r = chord / (2.0 * (a.abs() / 2.0).sin());
                    (a, r, r * a.abs())
                }
            } else {
                (0.0, 0.0, chord)
            };
            let xs = g
                .list(s, "x", s.i("x_count"))
                .into_iter()
                .map(|x| XSec {
                    dlat: pair(x, "dlat0", "dlat1"),
                    alt: pair(x, "alt0", "alt1"),
                    slope: pair(x, "slope0", "slope1"),
                })
                .collect();
            let fs = g
                .list(s, "f", s.i("f_count"))
                .into_iter()
                .map(|f| FSec { dlat: pair(f, "dlat0", "dlat1"), surface: f.i("i15") })
                .collect();
            let ws = g
                .list(s, "w", s.i("w_count"))
                .into_iter()
                .map(|w| WSec {
                    dlat: pair(w, "dlat0", "dlat1"),
                    height: pair(w, "height0", "height1"),
                    width: pair(w, "width0", "width1"),
                    wall_type: w.i("wall_type"),
                })
                .collect();
            segments.push(Segment { index, curve: curve && angle != 0.0, start, end, angle, radius, length, xs, fs, ws });
        }
        if segments.is_empty() {
            bail!("the track has no segments");
        }
        let objects = g
            .list(root, "tsoref", root.i("tsoref_count"))
            .into_iter()
            .map(|r| Placed {
                x: r.f("x"),
                y: r.f("y"),
                z: r.f("z"),
                heading: r.f("rot1"),
                file: g.child(r, "tso").and_then(|t| t.s("file")).unwrap_or("").to_string(),
            })
            .collect();
        let length = segments.iter().map(|s| s.length).sum();
        Ok(Self { night: root.i("night") != 0, segments, objects, length })
    }

    /// Bounding box of the centerline and objects: (min x, min y, max x, max y).
    pub fn bounds(&self) -> [f64; 4] {
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        let mut add = |x: f64, y: f64| {
            b[0] = b[0].min(x);
            b[1] = b[1].min(y);
            b[2] = b[2].max(x);
            b[3] = b[3].max(y);
        };
        for s in &self.segments {
            for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                for d in [s.min_dlat(t), s.max_dlat(t)] {
                    let p = s.point(t, d);
                    add(p[0], p[1]);
                }
            }
        }
        b
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Cubic Hermite from (0, a) with slope ma to (1, b) with slope mb (slopes per unit t).
fn hermite(a: f64, b: f64, ma: f64, mb: f64, t: f64) -> f64 {
    let t2 = t * t;
    let t3 = t2 * t;
    (2.0 * t3 - 3.0 * t2 + 1.0) * a + (t3 - 2.0 * t2 + t) * ma + (-2.0 * t3 + 3.0 * t2) * b + (t3 - t2) * mb
}

impl Segment {
    pub fn heading(&self, t: f64) -> f64 {
        if self.curve {
            self.start[2] + self.angle * t
        } else {
            (self.end[1] - self.start[1]).atan2(self.end[0] - self.start[0])
        }
    }

    /// World x, y of a point `t` (0..1) along the segment and `dlat` to the left.
    pub fn point(&self, t: f64, dlat: f64) -> [f64; 2] {
        let h = self.heading(t);
        let left = [-h.sin(), h.cos()];
        let c = if self.curve {
            let h0 = self.start[2];
            let l0 = [-h0.sin(), h0.cos()];
            let side = self.angle.signum(); // centre is to the left for a left turn
            let centre = [self.start[0] + side * self.radius * l0[0], self.start[1] + side * self.radius * l0[1]];
            [centre[0] - side * self.radius * left[0], centre[1] - side * self.radius * left[1]]
        } else {
            [lerp(self.start[0], self.end[0], t), lerp(self.start[1], self.end[1], t)]
        };
        [c[0] + dlat * left[0], c[1] + dlat * left[1]]
    }

    /// Length of the path at a sideways offset (shorter on the inside of a curve).
    pub fn length_at(&self, dlat: f64) -> f64 {
        if self.curve {
            ((self.radius - self.angle.signum() * dlat) * self.angle.abs()).max(0.0)
        } else {
            self.length
        }
    }

    /// The X sections at `t`: (dlat, height), sorted by dlat.
    pub fn profile(&self, t: f64) -> Vec<(f64, f64)> {
        let mut p: Vec<(f64, f64)> = self
            .xs
            .iter()
            .map(|x| {
                let d = lerp(x.dlat[0], x.dlat[1], t);
                let len = self.length_at(0.5 * (x.dlat[0] + x.dlat[1]));
                (d, hermite(x.alt[0], x.alt[1], x.slope[0] * len, x.slope[1] * len, t))
            })
            .collect();
        p.sort_by(|a, b| a.0.total_cmp(&b.0));
        p
    }

    /// Ground height at (t, dlat): linear between neighbouring X sections, flat beyond them.
    pub fn height(&self, t: f64, dlat: f64) -> f64 {
        height_in(&self.profile(t), dlat)
    }

    pub fn min_dlat(&self, t: f64) -> f64 {
        self.edge(t, f64::min, f64::MAX)
    }
    pub fn max_dlat(&self, t: f64) -> f64 {
        self.edge(t, f64::max, f64::MIN)
    }
    fn edge(&self, t: f64, pick: fn(f64, f64) -> f64, init: f64) -> f64 {
        let mut v = init;
        for d in self.xs.iter().map(|x| x.dlat).chain(self.fs.iter().map(|f| f.dlat)) {
            v = pick(v, lerp(d[0], d[1], t));
        }
        if v == init {
            0.0
        } else {
            v
        }
    }

    /// Banking at `t` in degrees: the cross slope of the ground under the centerline.
    /// Positive means the right side (outside of a left turn) is higher.
    pub fn banking(&self, t: f64) -> f64 {
        let p = self.profile(t);
        let k = p.iter().position(|q| q.0 > 0.0);
        let (a, b) = match k {
            Some(0) | None if p.len() < 2 => return 0.0,
            Some(0) => (p[0], p[1]),
            None => (p[p.len() - 2], p[p.len() - 1]),
            Some(k) => (p[k - 1], p[k]),
        };
        let dd = b.0 - a.0;
        if dd.abs() < 1e-6 {
            return 0.0;
        }
        ((a.1 - b.1) / dd).atan().to_degrees()
    }

    /// Ground strips at `t`: (from dlat, to dlat, surface), left to right order of dlat.
    pub fn strips(&self, t: f64) -> Vec<(f64, f64, i64)> {
        let mut f: Vec<(f64, i64)> = self.fs.iter().map(|f| (lerp(f.dlat[0], f.dlat[1], t), f.surface)).collect();
        f.sort_by(|a, b| a.0.total_cmp(&b.0));
        let right_edge = self.max_dlat(t);
        let mut out = Vec::new();
        for k in 0..f.len() {
            let to = if k + 1 < f.len() { f[k + 1].0 } else { right_edge };
            if to > f[k].0 + 1e-4 {
                out.push((f[k].0, to, f[k].1));
            }
        }
        out
    }
}

pub fn height_in(p: &[(f64, f64)], dlat: f64) -> f64 {
    match p.len() {
        0 => 0.0,
        1 => p[0].1,
        _ => {
            if dlat <= p[0].0 {
                return p[0].1;
            }
            for w in p.windows(2) {
                if dlat <= w[1].0 {
                    let dd = w[1].0 - w[0].0;
                    return if dd.abs() < 1e-9 { w[1].1 } else { lerp(w[0].1, w[1].1, (dlat - w[0].0) / dd) };
                }
            }
            p[p.len() - 1].1
        }
    }
}

/// Plain-English name for a ground surface type (Sandbox's F-section surface number).
pub fn surface_name(s: i64) -> &'static str {
    match s {
        1 => "Asphalt",
        2 => "Concrete",
        3 => "Paint",
        4 => "Grass",
        5 => "Dirt",
        6 => "Sand",
        7 => "Gravel",
        8 => "Water",
        10 => "Edge",
        12 => "Rumble strip",
        _ => "Other ground",
    }
}

/// Plain-English name for a wall type (from the 2003 community list, see HANDOFF.md).
pub fn wall_name(t: i64) -> &'static str {
    match t {
        2049 => "Hard wall",
        2050 => "Very hard wall",
        2051 => "Soft wall (springy)",
        2052 => "Soft wall",
        2053 => "Soft barrier",
        2054 => "Hedge",
        2055 => "Tire wall",
        2056 => "Hard wall (bouncy)",
        2057 | 2059 => "Metal guard rail",
        2058 => "Fence",
        2060 => "Rubber wall",
        10 => "Far left edge",
        _ => "Wall",
    }
}

pub const METRES_PER_MILE: f64 = 1609.344;
pub const FEET_PER_METRE: f64 = 1.0 / 0.3048;
