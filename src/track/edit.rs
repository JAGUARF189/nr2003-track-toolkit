//! Shape edits. Each function looks at the track and returns the field changes that make the
//! edit, so the caller can apply, undo and save them like any other change.
//!
//! - Length / turn / radius: the piece gets a new end; every later piece moves with it as one
//!   rigid block (and, if asked, the objects beside them), so the track stays joined.
//! - Banking: the racing surface tilts about its inside edge and everything further out rises or
//!   falls with it. The neighbouring pieces' ends are adjusted to match, so there's no step.
//!
//! Headings are kept in 0..2π, which Sandbox requires (sandbox.exe 0048C59D).

use super::graph::Graph;
use super::model::{height_in, Segment, Track};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::f64::consts::{PI, TAU};

#[derive(Clone, Debug)]
pub struct Change {
    pub id: i32,
    pub field: String,
    pub value: Value,
}

fn ch(id: i32, field: &str, v: f64) -> Change {
    Change { id, field: field.to_string(), value: json!(v) }
}

/// Into [0, 2π).
pub fn wrap_heading(h: f64) -> f64 {
    let w = h.rem_euclid(TAU);
    if w >= TAU || w.is_nan() {
        0.0
    } else {
        w
    }
}

fn wrap_pi(a: f64) -> f64 {
    let w = (a + PI).rem_euclid(TAU) - PI;
    if w <= -PI {
        w + TAU
    } else {
        w
    }
}

/// The new shape of a piece.
#[derive(Clone, Copy, Debug)]
pub enum Shape {
    Straight { length: f64 },
    /// `angle` in radians, positive = left turn; `radius` in metres at the centerline.
    Curve { angle: f64, radius: f64 },
}

/// End (x, y, heading) of a piece with this shape, starting at `start`.
pub fn end_pose(start: [f64; 3], shape: Shape) -> [f64; 3] {
    let h = start[2];
    match shape {
        Shape::Straight { length } => [start[0] + length * h.cos(), start[1] + length * h.sin(), h],
        Shape::Curve { angle, radius } => {
            let side = angle.signum();
            let l0 = [-h.sin(), h.cos()];
            let c = [start[0] + side * radius * l0[0], start[1] + side * radius * l0[1]];
            let h1 = h + angle;
            let l1 = [-h1.sin(), h1.cos()];
            [c[0] - side * radius * l1[0], c[1] - side * radius * l1[1], h1]
        }
    }
}

/// Rigid motion that takes the old end of the edited piece to its new end.
struct Motion {
    from: [f64; 2],
    to: [f64; 2],
    rot: f64,
}

impl Motion {
    fn point(&self, p: [f64; 2]) -> [f64; 2] {
        let (s, c) = self.rot.sin_cos();
        let d = [p[0] - self.from[0], p[1] - self.from[1]];
        [self.to[0] + c * d[0] - s * d[1], self.to[1] + s * d[0] + c * d[1]]
    }
}

fn nearest_piece(t: &Track, x: f64, y: f64) -> usize {
    let mut best = (f64::MAX, 0);
    for s in &t.segments {
        for k in 0..=8 {
            let p = s.point(k as f64 / 8.0, 0.0);
            let d = (p[0] - x).hypot(p[1] - y);
            if d < best.0 {
                best = (d, s.index);
            }
        }
    }
    best.1
}

/// Give piece `k` a new shape. Later pieces (and objects beside them, if `move_objects`) move
/// with its end so everything stays joined.
pub fn reshape(t: &Track, k: usize, shape: Shape, move_objects: bool) -> Result<Vec<Change>> {
    let Some(s) = t.segments.get(k) else { bail!("there is no piece {}", k + 1) };
    match shape {
        Shape::Straight { length } if !(0.01..=100_000.0).contains(&length) => bail!("length must be more than 1 cm"),
        Shape::Curve { radius, .. } if !(0.5..=1_000_000.0).contains(&radius) => bail!("radius must be at least 0.5 m"),
        Shape::Curve { angle, .. } if angle.abs() < 1e-5 || angle.abs() >= TAU => {
            bail!("a curve must turn more than 0 and less than 360 degrees")
        }
        _ => {}
    }
    let mut end = end_pose(s.start, shape);
    end[2] = wrap_heading(end[2]);
    let mut out = Vec::new();
    let curve = matches!(shape, Shape::Curve { .. });
    if curve != s.curve {
        out.push(Change { id: s.id, field: "type".into(), value: json!(curve as i64) });
    }
    out.push(ch(s.id, "ex", end[0]));
    out.push(ch(s.id, "ey", end[1]));
    out.push(ch(s.id, "eh", end[2]));

    let m = Motion { from: [s.end[0], s.end[1]], to: [end[0], end[1]], rot: wrap_pi(end[2] - s.end[2]) };
    for (j, n) in t.segments.iter().enumerate().skip(k + 1) {
        let (sp, sh) = if j == k + 1 {
            ([end[0], end[1]], end[2]) // exactly the new end, so the join is perfect
        } else {
            (m.point([n.start[0], n.start[1]]), wrap_heading(n.start[2] + m.rot))
        };
        let ep = m.point([n.end[0], n.end[1]]);
        out.push(ch(n.id, "sx", sp[0]));
        out.push(ch(n.id, "sy", sp[1]));
        out.push(ch(n.id, "sh", sh));
        out.push(ch(n.id, "ex", ep[0]));
        out.push(ch(n.id, "ey", ep[1]));
        out.push(ch(n.id, "eh", wrap_heading(n.end[2] + m.rot)));
    }
    if move_objects && (m.rot != 0.0 || m.from != m.to) {
        for o in &t.objects {
            if nearest_piece(t, o.x, o.y) > k {
                let p = m.point([o.x, o.y]);
                out.push(ch(o.id, "x", p[0]));
                out.push(ch(o.id, "y", p[1]));
                out.push(ch(o.id, "rot1", wrap_pi(o.heading + m.rot)));
            }
        }
    }
    Ok(out)
}

/// X sections of a piece at its start (`e` = 0) or end (1): (dlat, altitude, index in `xs`), by dlat.
fn ends(s: &Segment, e: usize) -> Vec<(f64, f64, usize)> {
    let mut p: Vec<(f64, f64, usize)> = s.xs.iter().enumerate().map(|(i, x)| (x.dlat[e], x.alt[e], i)).collect();
    p.sort_by(|a, b| a.0.total_cmp(&b.0));
    p
}

/// Index (into the sorted list) of the two X sections either side of the centerline.
fn around_centre(p: &[(f64, f64, usize)]) -> Option<(usize, usize)> {
    if p.len() < 2 {
        return None;
    }
    Some(match p.iter().position(|q| q.0 > 0.0) {
        Some(0) => (0, 1),
        None => (p.len() - 2, p.len() - 1),
        Some(k) => (k - 1, k),
    })
}

/// True if a left turn or a straight: the outside is the right side (lower dlat).
fn outside_is_right(s: &Segment) -> bool {
    !(s.curve && s.angle < 0.0)
}

/// Banking in degrees with the sign used by the editor: positive = outside of the turn higher
/// (the right side for straights).
pub fn banking_outside(s: &Segment, t: f64) -> f64 {
    let b = s.banking(t); // positive = right side higher
    if outside_is_right(s) {
        b
    } else {
        -b
    }
}

/// The two X sections either side of the centerline at one end: (outer moved side, pivot) as
/// indices into `s.xs`, plus the sorted end profile.
fn bank_pair(s: &Segment, e: usize) -> Result<(usize, usize, Vec<(f64, f64, usize)>)> {
    let p = ends(s, e);
    let Some((a, b)) = around_centre(&p) else { bail!("this piece has no cross-section to bank") };
    if p[b].0 - p[a].0 < 0.01 {
        bail!("the road surface here is too narrow to bank");
    }
    // The inside of the turn stays put; the outside (and everything beyond it) moves.
    Ok(if outside_is_right(s) { (p[a].2, p[b].2, p) } else { (p[b].2, p[a].2, p) })
}

/// Set banking (degrees, outside of the turn higher = positive) at the start and end of piece `k`.
///
/// The racing surface tilts about its inside edge. The outside point follows the inside edge's
/// own rises and falls plus an offset that blends evenly from the start banking to the end
/// banking; every point further out moves by the same amount (heights and grades), so walls,
/// aprons and run-off stay attached. The neighbouring pieces' joining ends follow, so the road
/// has no step and no change of grade at the joins.
pub fn set_banking(t: &Track, k: usize, start_deg: f64, end_deg: f64) -> Result<Vec<Change>> {
    let Some(s) = t.segments.get(k) else { bail!("there is no piece {}", k + 1) };
    for d in [start_deg, end_deg] {
        if !(-60.0..=60.0).contains(&d) {
            bail!("banking must be between -60 and 60 degrees");
        }
    }
    let (o0, pv0, p0) = bank_pair(s, 0)?;
    let (o1, pv1, p1) = bank_pair(s, 1)?;
    if o0 != o1 || pv0 != pv1 {
        bail!("the road's cross-section changes along this piece; set its banking at each end on the pieces either side");
    }
    let (o, pv) = (o0, pv0);
    let len = |i: usize| s.length_at(0.5 * (s.xs[i].dlat[0] + s.xs[i].dlat[1])).max(1e-6);
    let (xo, xp) = (&s.xs[o], &s.xs[pv]);
    let width = |e: usize| (xo.dlat[e] - xp.dlat[e]).abs();
    let off = [width(0) * start_deg.to_radians().tan(), width(1) * end_deg.to_radians().tan()];
    // Change of the outside point: height at each end, and slope (per unit t) at each end.
    let d_alt = [xp.alt[0] + off[0] - xo.alt[0], xp.alt[1] + off[1] - xo.alt[1]];
    let d_dt = [0, 1].map(|e| xp.slope[e] * len(pv) + (off[1] - off[0]) - xo.slope[e] * len(o));

    // Points that move: the outside point and everything beyond it.
    let right = outside_is_right(s);
    let dl_o = 0.5 * (xo.dlat[0] + xo.dlat[1]);
    let moves = |i: usize| {
        let d = 0.5 * (s.xs[i].dlat[0] + s.xs[i].dlat[1]);
        i == o || if right { d < dl_o } else { d > dl_o }
    };
    let _ = (p0, p1);
    let mut out = Vec::new();
    let mut new_x: Vec<(f64, f64, f64, f64)> = Vec::new(); // per xs: alt0, alt1, slope0, slope1
    for (i, x) in s.xs.iter().enumerate() {
        if moves(i) {
            let l = len(i);
            let v = (x.alt[0] + d_alt[0], x.alt[1] + d_alt[1], x.slope[0] + d_dt[0] / l, x.slope[1] + d_dt[1] / l);
            out.push(ch(x.id, "alt0", v.0));
            out.push(ch(x.id, "alt1", v.1));
            out.push(ch(x.id, "slope0", v.2));
            out.push(ch(x.id, "slope1", v.3));
            new_x.push(v);
        } else {
            new_x.push((x.alt[0], x.alt[1], x.slope[0], x.slope[1]));
        }
    }

    // Neighbours: heights follow this piece's new profile at the join; a cross-section point at
    // the same place also takes this piece's new grade there.
    let prof = |e: usize, new: bool| -> Vec<(f64, f64)> {
        let mut p: Vec<(f64, f64)> = s
            .xs
            .iter()
            .enumerate()
            .map(|(i, x)| (x.dlat[e], if new { if e == 0 { new_x[i].0 } else { new_x[i].1 } } else { x.alt[e] }))
            .collect();
        p.sort_by(|a, b| a.0.total_cmp(&b.0));
        p
    };
    let n = t.segments.len();
    let closed = n > 1 && {
        let (a, b) = (&t.segments[n - 1], &t.segments[0]);
        (a.end[0] - b.start[0]).hypot(a.end[1] - b.start[1]) < 0.5
    };
    let prev = if k > 0 { Some(k - 1) } else if closed { Some(n - 1) } else { None };
    let next = if k + 1 < n { Some(k + 1) } else if closed { Some(0) } else { None };
    // (neighbour, its end that joins us, our end at the join)
    for (j, their, ours) in [(prev, 1usize, 0usize), (next, 0, 1)].into_iter().filter_map(|(j, a, b)| j.map(|j| (j, a, b))) {
        if j == k {
            continue;
        }
        let (old, new) = (prof(ours, false), prof(ours, true));
        let (alt_f, slope_f) = if their == 1 { ("alt1", "slope1") } else { ("alt0", "slope0") };
        for x in &t.segments[j].xs {
            let d = x.dlat[their];
            let dh = height_in(&new, d) - height_in(&old, d);
            if dh.abs() > 1e-9 {
                out.push(ch(x.id, alt_f, x.alt[their] + dh));
            }
            if let Some(i) = s.xs.iter().position(|y| (y.dlat[ours] - d).abs() < 1e-3) {
                if moves(i) {
                    let ns = if ours == 0 { new_x[i].2 } else { new_x[i].3 };
                    if (ns - x.slope[their]).abs() > 1e-12 {
                        out.push(ch(x.id, slope_f, ns));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Apply changes to a graph (used by tests and tools; the GUI goes through its undo system).
pub fn apply(g: &mut Graph, changes: &[Change]) -> Result<()> {
    for c in changes {
        let Some(n) = g.node_mut(c.id) else { bail!("object {} is missing", c.id) };
        n.set(&c.field, c.value.clone())?;
    }
    Ok(())
}
