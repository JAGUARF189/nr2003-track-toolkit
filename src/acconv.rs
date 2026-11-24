//! Assetto Corsa -> NR2003, phase 2: an NR2003 track (.ptf) that follows an AC track.
//!
//! 1. **Centerline.** AC's AI line is the racing line; its file also stores the distance to the
//!    left and right track edges at every point. The NR2003 centerline is the middle between
//!    those edges, smoothed and resampled every metre.
//! 2. **Shape.** NR2003 pieces are straights and circular arcs joined without kinks, so the
//!    direction along the lap is a continuous piecewise-linear function of distance (flat on
//!    straights, sloped on arcs). That function is fitted to the centerline's direction, then
//!    the turn of every arc and the start position are adjusted by least squares so the pieces
//!    stay close to the AC centerline everywhere and the lap closes exactly. Pieces that stray
//!    too far are split and the fit repeated.
//! 3. **Height and width.** Road heights at the centre and both edges are sampled from AC's
//!    driving-surface meshes. Pieces are split further wherever NR2003's per-piece height curve
//!    (cubic along, straight across) or straight-line width change can't follow them.
//! 4. **Pieces.** Each piece gets cross-section points (outer grass edge, road edges, centre),
//!    ground strips (grass, asphalt, grass) and a wall at each outer edge, copied from a
//!    template track (stock hillside) so their textures and layouts are known-good. Texture
//!    coordinates are re-scaled to each new strip. Ground surfaces and walls from AC itself
//!    come in phase 3.

use crate::ac::{AcAiPoint, AcScene};
use crate::track::edit::wrap_heading;
use crate::track::graph::{Graph, Node};
use crate::track::model::Track;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::f64::consts::{PI, TAU};

/// How closely the NR2003 track follows the AC one.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Largest sideways distance between the NR2003 and AC centerlines (metres).
    pub position_m: f64,
    /// Largest height difference of the road (metres).
    pub height_m: f64,
    /// Largest road width difference (metres).
    pub width_m: f64,
    /// Grass beside the road on each side (metres), until phase 3 brings AC's own run-off.
    pub grass_m: f64,
    /// A wall at the outer edge of the grass on both sides.
    pub walls: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { position_m: 0.3, height_m: 0.1, width_m: 0.3, grass_m: 12.0, walls: true }
    }
}

/// The AC centerline, resampled every `ds` metres around the lap (not repeating the start).
#[derive(Clone, Debug)]
pub struct Centerline {
    pub ds: f64,
    pub pos: Vec<[f64; 2]>,
    /// Height of the AI line here (a hint for picking the road surface under it).
    pub z: Vec<f64>,
    /// Road width.
    pub width: Vec<f64>,
}

impl Centerline {
    pub fn len(&self) -> usize {
        self.pos.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pos.is_empty()
    }
    pub fn length(&self) -> f64 {
        self.ds * self.pos.len() as f64
    }
}

fn smooth_closed(v: &[f64], half: usize) -> Vec<f64> {
    let n = v.len();
    if n == 0 || half == 0 {
        return v.to_vec();
    }
    (0..n)
        .map(|i| {
            let mut s = 0.0;
            for k in 0..=2 * half {
                s += v[(i + n + k - half) % n];
            }
            s / (2 * half + 1) as f64
        })
        .collect()
}

/// The middle of the road along AC's AI line, smoothed and resampled every `ds` metres.
pub fn centerline(ai: &[AcAiPoint], ds: f64) -> Result<Centerline> {
    let n = ai.len();
    if n < 20 {
        bail!("the AC track has no usable AI line (ai\\fast_lane.ai)");
    }
    let mut cx = Vec::with_capacity(n);
    let mut cy = Vec::with_capacity(n);
    let mut cz = Vec::with_capacity(n);
    let mut w = Vec::with_capacity(n);
    let have_edges = ai.iter().filter(|p| p.left + p.right > 1.0).count() > n / 2;
    for k in 0..n {
        let (a, b) = (ai[(k + n - 1) % n].pos, ai[(k + 1) % n].pos);
        let (dx, dy) = ((b[0] - a[0]) as f64, (b[1] - a[1]) as f64);
        let len = dx.hypot(dy).max(1e-9);
        let (lx, ly) = (-dy / len, dx / len); // left of travel
        let p = ai[k];
        let (l, r) = if have_edges { (p.left as f64, p.right as f64) } else { (6.0, 6.0) };
        let shift = 0.5 * (l - r);
        cx.push(p.pos[0] as f64 + lx * shift);
        cy.push(p.pos[1] as f64 + ly * shift);
        cz.push(p.pos[2] as f64);
        w.push((l + r).max(3.0));
    }
    // smooth over about 8 m (the AI points are 1-2 m apart) and the width over about 30 m
    let spacing = ai.last().map_or(1.0, |p| p.distance as f64 / n as f64).max(0.3);
    let hp = ((4.0 / spacing).round() as usize).max(1);
    let hw = ((15.0 / spacing).round() as usize).max(1);
    let (cx, cy, cz, w) = (smooth_closed(&cx, hp), smooth_closed(&cy, hp), smooth_closed(&cz, hp), smooth_closed(&w, hw));
    // cumulative distance around the closed loop
    let mut s = vec![0.0; n + 1];
    for k in 0..n {
        let j = (k + 1) % n;
        s[k + 1] = s[k] + (cx[j] - cx[k]).hypot(cy[j] - cy[k]);
    }
    let total = s[n];
    let m = (total / ds).round().max(8.0) as usize;
    let ds = total / m as f64;
    let mut out = Centerline { ds, pos: Vec::with_capacity(m), z: Vec::with_capacity(m), width: Vec::with_capacity(m) };
    let mut k = 0;
    for i in 0..m {
        let t = i as f64 * ds;
        while k + 1 < n && s[k + 1] <= t {
            k += 1;
        }
        let j = (k + 1) % n;
        let f = ((t - s[k]) / (s[k + 1] - s[k]).max(1e-9)).clamp(0.0, 1.0);
        let lerp = |a: &[f64]| a[k] + (a[j] - a[k]) * f;
        out.pos.push([lerp(&cx), lerp(&cy)]);
        out.z.push(lerp(&cz));
        out.width.push(lerp(&w));
    }
    Ok(out)
}

// ------------------------------------------------------------------ the planar fit

/// One fitted piece: from distance `s0` for `len` metres, heading from `h0` to `h1` (radians,
/// unwrapped), starting at `p0`.
#[derive(Clone, Debug)]
pub struct Piece {
    pub s0: f64,
    pub len: f64,
    pub h0: f64,
    pub h1: f64,
    pub p0: [f64; 2],
}

impl Piece {
    pub fn straight(&self) -> bool {
        (self.h1 - self.h0).abs() < 1e-12
    }
    /// Position `d` metres into the piece.
    pub fn at(&self, d: f64) -> [f64; 2] {
        let dh = (self.h1 - self.h0) / self.len;
        let h = self.h0 + dh * d;
        if (dh * d).abs() < 1e-9 {
            [self.p0[0] + d * self.h0.cos(), self.p0[1] + d * self.h0.sin()]
        } else {
            [self.p0[0] + (h.sin() - self.h0.sin()) / dh, self.p0[1] + (self.h0.cos() - h.cos()) / dh]
        }
    }
    pub fn end(&self) -> [f64; 2] {
        self.at(self.len)
    }
}

/// Heading of the centerline at every sample (unwrapped), and the lap's total turn (±2π).
fn headings(cl: &Centerline) -> (Vec<f64>, f64) {
    let n = cl.len();
    let mut h = Vec::with_capacity(n + 1);
    let mut prev: Option<f64> = None;
    for i in 0..=n {
        let (a, b) = (cl.pos[(i + n - 1) % n], cl.pos[(i + 1) % n]);
        let raw = (b[1] - a[1]).atan2(b[0] - a[0]);
        let v = match prev {
            Some(p) => p + (raw - p + PI).rem_euclid(TAU) - PI,
            None => raw,
        };
        h.push(v);
        prev = Some(v);
    }
    let total = ((h[n] - h[0]) / TAU).round() * TAU;
    h[n] = h[0] + total;
    (h, total)
}

/// Knots (sample indices) of a piecewise-linear fit of `h` within `tol` (Douglas-Peucker).
fn split_knots(h: &[f64], tol: f64) -> Vec<usize> {
    let mut knots = vec![0, h.len() - 1];
    let mut stack = vec![(0usize, h.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (mut worst, mut at) = (0.0, a);
        for i in a + 1..b {
            let line = h[a] + (h[b] - h[a]) * (i - a) as f64 / (b - a) as f64;
            let e = (h[i] - line).abs();
            if e > worst {
                worst = e;
                at = i;
            }
        }
        if worst > tol {
            knots.push(at);
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    knots.sort_unstable();
    knots.dedup();
    knots
}

struct Fit {
    /// Knots as sample indices, 0 .. n (n = the start again).
    knots: Vec<usize>,
    straight: Vec<bool>,
    /// Heading change of each piece (0 on straights).
    turn: Vec<f64>,
    h0: f64,
    p0: [f64; 2],
}

impl Fit {
    fn pieces(&self, ds: f64) -> Vec<Piece> {
        let mut out = Vec::with_capacity(self.turn.len());
        let (mut h, mut p) = (self.h0, self.p0);
        for k in 0..self.turn.len() {
            let len = (self.knots[k + 1] - self.knots[k]) as f64 * ds;
            let piece = Piece { s0: self.knots[k] as f64 * ds, len, h0: h, h1: h + self.turn[k], p0: p };
            p = piece.end();
            h = piece.h1;
            out.push(piece);
        }
        out
    }

    /// Residuals: distance to the centerline at every `step`-th sample, then the closing gap
    /// (weighted).
    fn residuals(&self, cl: &Centerline, step: usize, out: &mut Vec<f64>) {
        out.clear();
        let pieces = self.pieces(cl.ds);
        let mut k = 0;
        for i in (0..cl.len()).step_by(step) {
            while k + 1 < pieces.len() && self.knots[k + 1] <= i {
                k += 1;
            }
            let p = pieces[k].at((i - self.knots[k]) as f64 * cl.ds);
            out.push(p[0] - cl.pos[i][0]);
            out.push(p[1] - cl.pos[i][1]);
        }
        let end = pieces.last().map_or(self.p0, |p| p.end());
        out.push(50.0 * (end[0] - self.p0[0]));
        out.push(50.0 * (end[1] - self.p0[1]));
    }

    /// Worst distance from the centerline and the sample where it is.
    fn worst(&self, cl: &Centerline) -> (f64, usize, f64) {
        let pieces = self.pieces(cl.ds);
        let (mut worst, mut at, mut sum) = (0.0, 0, 0.0);
        let mut k = 0;
        for i in 0..cl.len() {
            while k + 1 < pieces.len() && self.knots[k + 1] <= i {
                k += 1;
            }
            let p = pieces[k].at((i - self.knots[k]) as f64 * cl.ds);
            let e = (p[0] - cl.pos[i][0]).hypot(p[1] - cl.pos[i][1]);
            sum += e;
            if e > worst {
                worst = e;
                at = i;
            }
        }
        (worst, at, sum / cl.len() as f64)
    }
}

/// Solve the square system `a x = b` (Gaussian elimination with partial pivoting).
fn solve(mut a: Vec<f64>, mut b: Vec<f64>, n: usize) -> Option<Vec<f64>> {
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| a[i * n + c].abs().total_cmp(&a[j * n + c].abs()))?;
        if a[p * n + c].abs() < 1e-14 {
            return None;
        }
        if p != c {
            for k in 0..n {
                a.swap(c * n + k, p * n + k);
            }
            b.swap(c, p);
        }
        for r in c + 1..n {
            let f = a[r * n + c] / a[c * n + c];
            if f != 0.0 {
                for k in c..n {
                    a[r * n + k] -= f * a[c * n + k];
                }
                b[r] -= f * b[c];
            }
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let mut s = b[r];
        for k in r + 1..n {
            s -= a[r * n + k] * x[k];
        }
        x[r] = s / a[r * n + r];
    }
    Some(x)
}

/// Least-squares refinement of the arcs' turns and the start (Levenberg-Marquardt), keeping
/// the lap's total turn exact by letting the biggest arc take up the difference.
fn refine(fit: &mut Fit, cl: &Centerline, total: f64, iterations: usize) {
    let curves: Vec<usize> = (0..fit.turn.len()).filter(|&k| !fit.straight[k]).collect();
    if curves.is_empty() {
        return;
    }
    let dep = *curves.iter().max_by(|&&a, &&b| fit.turn[a].abs().total_cmp(&fit.turn[b].abs())).unwrap();
    let free: Vec<usize> = curves.iter().copied().filter(|&k| k != dep).collect();
    let np = free.len() + 3; // free turns, start heading, start x, start y
    let step = if cl.len() > 4000 { 2 } else { 1 };
    let set = |fit: &mut Fit, p: &[f64]| {
        for (j, &k) in free.iter().enumerate() {
            fit.turn[k] = p[j];
        }
        fit.h0 = p[free.len()];
        fit.p0 = [p[free.len() + 1], p[free.len() + 2]];
        let others: f64 = (0..fit.turn.len()).filter(|&k| k != dep).map(|k| fit.turn[k]).sum();
        fit.turn[dep] = total - others;
    };
    let mut p: Vec<f64> = free.iter().map(|&k| fit.turn[k]).collect();
    p.extend([fit.h0, fit.p0[0], fit.p0[1]]);
    set(fit, &p);
    let mut r = Vec::new();
    fit.residuals(cl, step, &mut r);
    let mut cost: f64 = r.iter().map(|x| x * x).sum();
    let mut lambda = 1e-3;
    let mut rp = Vec::new();
    for _ in 0..iterations {
        // Jacobian by finite differences (columns)
        let m = r.len();
        let mut jac = vec![0.0; m * np];
        for c in 0..np {
            let eps = if c == free.len() + 1 || c == free.len() + 2 { 1e-3 } else { 1e-6 };
            let mut q = p.clone();
            q[c] += eps;
            set(fit, &q);
            fit.residuals(cl, step, &mut rp);
            for i in 0..m {
                jac[i * np + c] = (rp[i] - r[i]) / eps;
            }
        }
        set(fit, &p);
        let mut jtj = vec![0.0; np * np];
        let mut jtr = vec![0.0; np];
        for i in 0..m {
            let row = &jac[i * np..(i + 1) * np];
            for a in 0..np {
                if row[a] == 0.0 {
                    continue;
                }
                jtr[a] += row[a] * r[i];
                for b in a..np {
                    jtj[a * np + b] += row[a] * row[b];
                }
            }
        }
        for a in 0..np {
            for b in 0..a {
                jtj[a * np + b] = jtj[b * np + a];
            }
        }
        let mut improved = false;
        for _ in 0..8 {
            let mut aug = jtj.clone();
            for a in 0..np {
                aug[a * np + a] += lambda * (1.0 + jtj[a * np + a]);
            }
            let Some(delta) = solve(aug, jtr.iter().map(|x| -x).collect(), np) else {
                lambda *= 10.0;
                continue;
            };
            let q: Vec<f64> = p.iter().zip(&delta).map(|(a, d)| a + d).collect();
            set(fit, &q);
            fit.residuals(cl, step, &mut rp);
            let c2: f64 = rp.iter().map(|x| x * x).sum();
            if c2 < cost {
                p = q;
                std::mem::swap(&mut r, &mut rp);
                let gain = (cost - c2) / cost.max(1e-12);
                cost = c2;
                lambda = (lambda * 0.3).max(1e-9);
                improved = gain > 1e-7;
                break;
            }
            lambda *= 10.0;
        }
        set(fit, &p);
        if !improved {
            break;
        }
    }
}

/// Straights: pieces whose heading barely changes.
const STRAIGHT_TURN: f64 = 0.35 * PI / 180.0;

fn classify(h: &[f64], knots: &[usize]) -> (Vec<bool>, Vec<f64>) {
    let mut straight = Vec::new();
    let mut turn = Vec::new();
    for w in knots.windows(2) {
        let d = h[w[1]] - h[w[0]];
        let s = d.abs() < STRAIGHT_TURN;
        straight.push(s);
        turn.push(if s { 0.0 } else { d });
    }
    (straight, turn)
}

/// Fit straights and arcs to the centerline. Returns the pieces, the worst and mean distance
/// from the AC centerline.
pub fn fit_plan(cl: &Centerline, tol_m: f64) -> Result<(Vec<Piece>, f64, f64)> {
    let n = cl.len();
    let (h, total) = headings(cl);
    if total.abs() < 1.0 {
        bail!("the AI line doesn't go round a lap");
    }
    // a first fit of the heading within ~0.8°, then refine and split where it strays
    let mut knots = split_knots(&h, 0.8f64.to_radians());
    for _ in 0..12 {
        let (straight, turn) = classify(&h, &knots);
        let mut fit = Fit { knots: knots.clone(), straight, turn, h0: h[0], p0: cl.pos[0] };
        refine(&mut fit, cl, total, 25);
        let (worst, _, mean) = fit.worst(cl);
        // split every piece whose own worst error is too big, at that worst sample
        let pieces = fit.pieces(cl.ds);
        let mut add = Vec::new();
        let mut k = 0;
        let mut piece_worst: Vec<(f64, usize)> = vec![(0.0, 0); pieces.len()];
        for i in 0..n {
            while k + 1 < pieces.len() && fit.knots[k + 1] <= i {
                k += 1;
            }
            let p = pieces[k].at((i - fit.knots[k]) as f64 * cl.ds);
            let e = (p[0] - cl.pos[i][0]).hypot(p[1] - cl.pos[i][1]);
            if e > piece_worst[k].0 {
                piece_worst[k] = (e, i);
            }
        }
        for (k, (e, i)) in piece_worst.iter().enumerate() {
            let (a, b) = (fit.knots[k], fit.knots[k + 1]);
            if *e > tol_m && b - a >= 6 {
                // split near the worst point, but not right at the ends
                add.push((*i).clamp(a + 2, b - 2));
            }
        }
        if worst <= tol_m || add.is_empty() {
            let mut pieces = fit.pieces(cl.ds);
            close_exactly(&mut pieces, cl.length());
            return Ok((pieces, worst, mean));
        }
        knots.extend(add);
        knots.sort_unstable();
        knots.dedup();
    }
    let (straight, turn) = classify(&h, &knots);
    let mut fit = Fit { knots, straight, turn, h0: h[0], p0: cl.pos[0] };
    refine(&mut fit, cl, total, 40);
    let (worst, _, mean) = fit.worst(cl);
    let mut pieces = fit.pieces(cl.ds);
    close_exactly(&mut pieces, cl.length());
    Ok((pieces, worst, mean))
}

/// Take up the last few millimetres of closing gap by nudging every piece's start a little
/// (spread along the lap), so the last piece ends exactly where the first starts.
fn close_exactly(pieces: &mut [Piece], total: f64) {
    let Some(last) = pieces.last() else { return };
    let end = last.end();
    let gap = [pieces[0].p0[0] - end[0], pieces[0].p0[1] - end[1]];
    for p in pieces.iter_mut() {
        let f = p.s0 / total;
        p.p0 = [p.p0[0] + gap[0] * f, p.p0[1] + gap[1] * f];
    }
}

// ------------------------------------------------------------------ room beside the road

/// How much room there is beside the road at every centerline sample, before another part of
/// the track (one that isn't the stretch just before or after): (right, left) in metres from the
/// road edge to halfway to that other road's edge. Run-off and walls stop there, so parallel
/// stretches (Monaco's pit straight and harbour road) don't overlap.
pub fn room_beside(cl: &Centerline, max: f64) -> (Vec<f64>, Vec<f64>) {
    let n = cl.len();
    let cell = 8.0;
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, p) in cl.pos.iter().enumerate() {
        grid.entry(((p[0] / cell).floor() as i32, (p[1] / cell).floor() as i32)).or_default().push(i);
    }
    let reach = max * 2.0 + cl.width.iter().fold(0.0f64, |a, &b| a.max(b)) + 4.0;
    let cells = (reach / cell).ceil() as i32;
    let skip = (60.0 / cl.ds).round() as usize; // the same stretch: 60 m either way
    let mut right = vec![max; n];
    let mut left = vec![max; n];
    for i in 0..n {
        let p = cl.pos[i];
        let (a, b) = (cl.pos[(i + n - 1) % n], cl.pos[(i + 1) % n]);
        let (tx, ty) = (b[0] - a[0], b[1] - a[1]);
        let tl = tx.hypot(ty).max(1e-9);
        let (tx, ty) = (tx / tl, ty / tl);
        let (lx, ly) = (-ty, tx);
        let half = 0.5 * cl.width[i];
        let (gx, gy) = ((p[0] / cell).floor() as i32, (p[1] / cell).floor() as i32);
        for cx in gx - cells..=gx + cells {
            for cy in gy - cells..=gy + cells {
                let Some(list) = grid.get(&(cx, cy)) else { continue };
                for &j in list {
                    let ds = (i as isize - j as isize).unsigned_abs();
                    if ds.min(n - ds) <= skip {
                        continue;
                    }
                    let (dx, dy) = (cl.pos[j][0] - p[0], cl.pos[j][1] - p[1]);
                    let lon = dx * tx + dy * ty;
                    let lat = dx * lx + dy * ly;
                    let other = 0.5 * cl.width[j];
                    if lon.abs() > other + max || lat.abs() < 1e-6 {
                        continue;
                    }
                    // free ground between the two road edges, shared half each
                    let free = ((lat.abs() - half - other) * 0.5 - 0.75).max(0.0);
                    if lat > 0.0 {
                        left[i] = left[i].min(free);
                    } else {
                        right[i] = right[i].min(free);
                    }
                }
            }
        }
    }
    (right, left)
}

// ------------------------------------------------------------------ road heights from AC

/// Heights of AC's driving surfaces (not walls), found by looking straight down.
pub struct RoadHeights {
    cell: f64,
    tris: Vec<[f32; 9]>,
    grid: HashMap<(i32, i32), Vec<u32>>,
}

impl RoadHeights {
    pub fn new(s: &AcScene) -> Self {
        let cell = 4.0;
        let mut tris = Vec::new();
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for m in &s.meshes {
            let Some(sf) = m.surface.and_then(|k| s.surfaces.get(k)) else { continue };
            if sf.key == "WALL" {
                continue;
            }
            for t in m.indices.chunks_exact(3) {
                let v = |i: u32| {
                    let b = i as usize * 8;
                    [m.verts[b], m.verts[b + 1], m.verts[b + 2]]
                };
                let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
                let id = tris.len() as u32;
                tris.push([a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]]);
                let lo = |k: usize| a[k].min(b[k]).min(c[k]) as f64;
                let hi = |k: usize| a[k].max(b[k]).max(c[k]) as f64;
                let (x0, x1) = ((lo(0) / cell).floor() as i32, (hi(0) / cell).floor() as i32);
                let (y0, y1) = ((lo(1) / cell).floor() as i32, (hi(1) / cell).floor() as i32);
                if (x1 - x0) > 64 || (y1 - y0) > 64 {
                    continue; // a giant triangle (terrain); skip rather than flood the grid
                }
                for gx in x0..=x1 {
                    for gy in y0..=y1 {
                        grid.entry((gx, gy)).or_default().push(id);
                    }
                }
            }
        }
        Self { cell, tris, grid }
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// Height of the driving surface under (x, y) closest to `near`, if any.
    pub fn at(&self, x: f64, y: f64, near: f64) -> Option<f64> {
        let key = ((x / self.cell).floor() as i32, (y / self.cell).floor() as i32);
        let mut best: Option<f64> = None;
        for &id in self.grid.get(&key)? {
            let t = &self.tris[id as usize];
            let (ax, ay, az) = (t[0] as f64, t[1] as f64, t[2] as f64);
            let (bx, by, bz) = (t[3] as f64, t[4] as f64, t[5] as f64);
            let (cx, cy, cz) = (t[6] as f64, t[7] as f64, t[8] as f64);
            let d = (by - cy) * (ax - cx) + (cx - bx) * (ay - cy);
            if d.abs() < 1e-12 {
                continue;
            }
            let l1 = ((by - cy) * (x - cx) + (cx - bx) * (y - cy)) / d;
            let l2 = ((cy - ay) * (x - cx) + (ax - cx) * (y - cy)) / d;
            let l3 = 1.0 - l1 - l2;
            if l1 < -1e-6 || l2 < -1e-6 || l3 < -1e-6 {
                continue;
            }
            let z = l1 * az + l2 * bz + l3 * cz;
            if best.is_none_or(|b| (z - near).abs() < (b - near).abs()) {
                best = Some(z);
            }
        }
        best.filter(|z| (z - near).abs() < 2.0)
    }
}

// ------------------------------------------------------------------ building the track

/// What the conversion did, in numbers.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub pieces: usize,
    pub straights: usize,
    pub curves: usize,
    /// Distance between the NR2003 and AC centerlines: worst and average (metres).
    pub worst_m: f64,
    pub mean_m: f64,
    pub length_ac: f64,
    pub length_nr: f64,
    /// Share of road-height samples found on AC's driving surfaces (the rest follow the AI line).
    pub heights_found: f64,
    /// Height difference of the road centre: worst and average (metres).
    pub worst_height_m: f64,
    pub mean_height_m: f64,
    /// The checker's findings that would stop the track loading (should be none).
    pub crashes: Vec<String>,
    /// Subtracted from AC's x, y to centre the NR2003 track (add it back to compare).
    pub offset: [f64; 2],
    pub warnings: usize,
}

/// A converted track: the .ptf graph and the numbers.
pub struct Converted {
    pub graph: Graph,
    pub report: Report,
}

/// Template nodes to copy pieces from (found in a stock track such as hillside).
struct Templates {
    seg: Node,
    x: Node,
    asphalt: i32,
    grass: i32,
    wall: i32,
}

fn find_templates(g: &Graph) -> Result<Templates> {
    let root = g.root();
    let seg = g.child(root, "seg[0]").context("the template has no track pieces")?.clone();
    let mut ids: Vec<i32> = g.nodes.keys().copied().collect();
    ids.sort_unstable();
    let tex_of = |n: &Node| -> Option<String> {
        let app = g.child(n, "appearance")?;
        if app.r("tex[2]").is_some() {
            return None; // two textures: keep it simple
        }
        g.child(app, "tex[0]")?.s("file").map(|s| s.to_ascii_lowercase())
    };
    let x = ids
        .iter()
        .filter_map(|id| g.node(*id))
        .find(|n| n.class == "X_SectionDescriptor" && n.i("a_mode") == 0 && n.i("b_mode") == 0)
        .context("the template has no plain cross-section point")?
        .clone();
    let strip = |surface: i64, word: &str| -> Option<i32> {
        ids.iter().filter_map(|id| g.node(*id)).find(|n| {
            n.class == "F_SectionDescriptor"
                && n.i("i15") == surface
                && n.get("coords").is_some()
                && n.i("layer0.mode0") == 0
                && n.i("layer0.mode1") == 0
                && tex_of(n).is_some_and(|t| t.contains(word))
        })
        .map(|n| n.id)
    };
    let asphalt = strip(1, "asphalt").or_else(|| strip(1, "")).context("the template has no asphalt strip")?;
    let grass = strip(4, "grass").or_else(|| strip(4, "")).unwrap_or(asphalt);
    let wall = ids
        .iter()
        .filter_map(|id| g.node(*id))
        .find(|n| n.class == "W_SectionDescriptor" && n.i("wall_type") == 2049 && n.i("sub_count") == 1 && n.i("i49") == 0)
        .or_else(|| ids.iter().filter_map(|id| g.node(*id)).find(|n| n.class == "W_SectionDescriptor"))
        .context("the template has no wall")?
        .id;
    Ok(Templates { seg, x, asphalt, grass, wall })
}

fn hex_doubles(v: &[f64]) -> String {
    let mut s = String::with_capacity(v.len() * 16);
    for x in v {
        for b in x.to_le_bytes() {
            s.push_str(&format!("{b:02x}"));
        }
    }
    s
}

fn doubles_hex(s: &str) -> Option<Vec<f64>> {
    let b: Vec<u8> = (0..s.len() / 2).map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()).collect::<Option<_>>()?;
    Some(b.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect())
}

/// Corner geometry of a strip or wall face, for re-scaling texture coordinates:
/// sideways position of the corners (start-right, end-right, end-left, start-left) and the
/// lengths of its right and left edges.
#[derive(Clone, Copy)]
struct Corners {
    d: [f64; 4],
    len_r: f64,
    len_l: f64,
}

/// Re-scale a face's texture coordinates from the template's corners to new ones: along the
/// piece by edge length; across by position, unless the template stretches the picture once
/// across (0..1), which is kept. `across_world`: across means sideways position (strips), not
/// height (walls).
fn remap(a: &[f64], t: Corners, n: Corners, across_world: bool) -> Vec<f64> {
    if a.len() < 4 {
        return a.to_vec();
    }
    let stretched = |p: f64, q: f64| ((p - q).abs() - 1.0).abs() < 1e-6 && (p.abs() < 1e-6 || (p - 1.0).abs() < 1e-6);
    let (mut a0, mut a3) = (a[0], a[3]);
    if across_world && !stretched(a[0], a[3]) && (t.d[3] - t.d[0]).abs() > 1e-6 {
        let rate = (a[3] - a[0]) / (t.d[3] - t.d[0]);
        a0 = a[0] + rate * (n.d[0] - t.d[0]);
        a3 = a[3] + rate * (n.d[3] - t.d[3]);
    }
    let a1 = a0 + (a[1] - a[0]) * n.len_r / t.len_r.max(1e-6);
    let a2 = a3 + (a[2] - a[3]) * n.len_l / t.len_l.max(1e-6);
    vec![a0, a1, a2, a3]
}

/// Template geometry of strip/wall `id` (from the template track's model).
fn template_corners(tt: &Track, id: i32) -> Option<Corners> {
    for s in &tt.segments {
        if let Some(k) = s.fs.iter().position(|f| f.id == id) {
            let f = &s.fs[k];
            let next = |e: usize| s.fs.iter().map(|x| x.dlat[e]).filter(|&x| x > f.dlat[e] + 1e-6).fold(f64::MAX, f64::min);
            let left = |e: usize| {
                let v = next(e);
                if v == f64::MAX { s.max_dlat(e as f64) } else { v }
            };
            let d = [f.dlat[0], f.dlat[1], left(1), left(0)];
            return Some(Corners { d, len_r: s.length_at(0.5 * (d[0] + d[1])), len_l: s.length_at(0.5 * (d[2] + d[3])) });
        }
        if let Some(w) = s.ws.iter().find(|w| w.id == id) {
            let len = s.length_at(0.5 * (w.dlat[0] + w.dlat[1]));
            return Some(Corners { d: [w.dlat[0], w.dlat[1], w.dlat[1], w.dlat[0]], len_r: len, len_l: len });
        }
    }
    None
}

/// Copy strip/wall `src` with its own appearances and texture coordinates (textures shared),
/// re-scaling the coordinates to the new corners. Returns the new id.
fn clone_part(g: &Graph, out: &mut HashMap<i32, Node>, next: &mut i32, src: i32, t: Corners, n: Corners) -> Result<i32> {
    let node = g.node(src).context("template part missing")?;
    let is_wall = node.class == "W_SectionDescriptor";
    let mut copy = node.clone();
    copy.id = *next;
    *next += 1;
    for f in &mut copy.fields {
        if f.1 != "obj" {
            continue;
        }
        let Some(c) = f.2.as_i64().map(|c| c as i32).filter(|&c| c != 0) else { continue };
        let Some(child) = g.node(c) else { continue };
        match child.class.as_str() {
            "AppearanceDescriptor" => {
                let mut a = child.clone();
                a.id = *next;
                *next += 1;
                f.2 = json!(a.id);
                out.insert(a.id, a);
            }
            "TextureCoordsDescriptor" => {
                let mut tc = child.clone();
                tc.id = *next;
                *next += 1;
                for k in 0..24 {
                    let name = format!("a[{k}]");
                    let Some(hex) = tc.s(&name).map(str::to_string) else { continue };
                    let Some(vals) = doubles_hex(&hex) else { continue };
                    // strips: even = across, odd = along; walls: even = along, odd = height
                    let across_world = !is_wall && k % 2 == 0;
                    let new = if is_wall && k % 2 == 1 { vals.clone() } else { remap(&vals, t, n, across_world) };
                    tc.set(&name, json!(hex_doubles(&new)))?;
                }
                f.2 = json!(tc.id);
                out.insert(tc.id, tc);
            }
            _ => {}
        }
    }
    let id = copy.id;
    out.insert(id, copy);
    Ok(id)
}

/// Sampled profile along the fitted pieces: centre and edge heights and the width, every ds.
struct Profile {
    s: Vec<f64>,
    hc: Vec<f64>,
    hr: Vec<f64>,
    hl: Vec<f64>,
    w: Vec<f64>,
}

/// Hermite height along a piece from (h0, slope m0) to (h1, slope m1) over `len`.
fn hermite(h0: f64, h1: f64, m0: f64, m1: f64, len: f64, t: f64) -> f64 {
    let (t2, t3) = (t * t, t * t * t);
    (2.0 * t3 - 3.0 * t2 + 1.0) * h0 + (t3 - 2.0 * t2 + t) * m0 * len + (-2.0 * t3 + 3.0 * t2) * h1 + (t3 - t2) * m1 * len
}

/// Slope (rise per metre) of `h` at sample `i` (closed loop).
fn slope_at(h: &[f64], i: usize, ds: f64) -> f64 {
    let n = h.len();
    let span = 3;
    (h[(i + span) % n] - h[(i + n - span) % n]) / (2.0 * span as f64 * ds)
}

/// Convert an AC scene into an NR2003 track graph, using `template` (a stock .ptf graph) for
/// the pieces' strips, walls and track settings.
pub fn convert(scene: &AcScene, template: &Graph, opt: &Options) -> Result<Converted> {
    let mut report = Report::default();
    let cl = centerline(&scene.ai, 1.0)?;
    report.length_ac = cl.length();
    let (mut pieces, worst, mean) = fit_plan(&cl, opt.position_m)?;
    report.worst_m = worst;
    report.mean_m = mean;

    // --- heights and widths along the fitted track, every metre
    let roads = RoadHeights::new(scene);
    let n = cl.len();
    let ds = cl.ds;
    let mut prof = Profile { s: Vec::with_capacity(n), hc: Vec::new(), hr: Vec::new(), hl: Vec::new(), w: Vec::new() };
    let mut found = 0usize;
    let mut k = 0;
    for i in 0..n {
        let s = i as f64 * ds;
        while k + 1 < pieces.len() && pieces[k + 1].s0 <= s + 1e-9 {
            k += 1;
        }
        let pc = &pieces[k];
        let d = s - pc.s0;
        let p = pc.at(d);
        let h = pc.h0 + (pc.h1 - pc.h0) * d / pc.len;
        let (lx, ly) = (-h.sin(), h.cos());
        let half = 0.5 * cl.width[i];
        let near = cl.z[i];
        let c = roads.at(p[0], p[1], near);
        let hc = c.unwrap_or(near);
        // edges: only believable heights (banking up to about 17°; a kerb or wall top isn't the road)
        let bank_ok = |z: &f64| (z - hc).abs() <= 0.3 * half + 0.3;
        let r = roads.at(p[0] - lx * half * 0.95, p[1] - ly * half * 0.95, hc).filter(bank_ok);
        let l = roads.at(p[0] + lx * half * 0.95, p[1] + ly * half * 0.95, hc).filter(bank_ok);
        found += usize::from(c.is_some()) + usize::from(r.is_some()) + usize::from(l.is_some());
        prof.s.push(s);
        prof.hc.push(hc);
        prof.hr.push(r.unwrap_or(hc));
        prof.hl.push(l.unwrap_or(hc));
        prof.w.push(cl.width[i]);
    }
    report.heights_found = found as f64 / (3 * n) as f64;
    // smooth over a few metres (mesh facets)
    prof.hc = smooth_closed(&prof.hc, 2);
    prof.hr = smooth_closed(&prof.hr, 2);
    prof.hl = smooth_closed(&prof.hl, 2);
    // the road's lowest point becomes height 0
    let base = prof.hc.iter().chain(&prof.hr).chain(&prof.hl).fold(f64::MAX, |a, &b| a.min(b));
    for v in prof.hc.iter_mut().chain(prof.hr.iter_mut()).chain(prof.hl.iter_mut()) {
        *v -= base;
    }

    // --- split pieces where the height curve or the width can't follow
    let sample = |s: f64| ((s / ds).round() as usize) % n;
    let mut worst_h: f64 = 0.0;
    let mut i = 0;
    while i < pieces.len() {
        let pc = pieces[i].clone();
        let (a, b) = (sample(pc.s0), sample(pc.s0 + pc.len));
        let b_idx = if b <= a { b + n } else { b };
        let mut bad: Option<(f64, usize)> = None;
        for (h, tol) in [(&prof.hc, opt.height_m), (&prof.hr, opt.height_m), (&prof.hl, opt.height_m), (&prof.w, opt.width_m)] {
            let is_width = std::ptr::eq(h, &prof.w);
            let (h0, h1) = (h[a], h[b % n]);
            let (m0, m1) = (slope_at(h, a, ds), slope_at(h, b % n, ds));
            for j in a + 1..b_idx {
                let t = (j - a) as f64 / (b_idx - a) as f64;
                let model = if is_width { h0 + (h1 - h0) * t } else { hermite(h0, h1, m0, m1, pc.len, t) };
                let e = (model - h[j % n]).abs();
                if !is_width {
                    worst_h = worst_h.max(e.min(tol * 4.0));
                }
                if e > tol && bad.is_none_or(|x| e / tol > x.0) {
                    bad = Some((e / tol, j));
                }
            }
        }
        // also: arcs over 90° are split (stock tracks never have them)
        let over = (pc.h1 - pc.h0).abs() > 89f64.to_radians();
        let cut = match bad {
            Some((_, j)) if b_idx - a >= 8 => Some((j.clamp(a + 3, b_idx - 3) - a) as f64 * ds),
            _ if over => Some(pc.len * 0.5),
            _ => None,
        };
        if let Some(d) = cut {
            let mid_h = pc.h0 + (pc.h1 - pc.h0) * d / pc.len;
            let first = Piece { s0: pc.s0, len: d, h0: pc.h0, h1: mid_h, p0: pc.p0 };
            let second = Piece { s0: pc.s0 + d, len: pc.len - d, h0: mid_h, h1: pc.h1, p0: first.end() };
            pieces[i] = first;
            pieces.insert(i + 1, second);
            continue; // look at the first half again
        }
        i += 1;
    }
    // the height difference that's left (centre of the road), over all pieces
    worst_h = 0.0;
    let mut sum_h = 0.0;
    for pc in &pieces {
        let (a, b) = (sample(pc.s0), sample(pc.s0 + pc.len));
        let b_idx = if b <= a { b + n } else { b };
        let h = &prof.hc;
        let (m0, m1) = (slope_at(h, a, ds).clamp(-0.3, 0.3), slope_at(h, b % n, ds).clamp(-0.3, 0.3));
        for j in a..b_idx {
            let t = (j - a) as f64 / (b_idx - a).max(1) as f64;
            let e = (hermite(h[a], h[b % n], m0, m1, pc.len, t) - h[j % n]).abs();
            worst_h = worst_h.max(e);
            sum_h += e;
        }
    }
    report.worst_height_m = worst_h;
    report.mean_height_m = sum_h / n as f64;

    // --- room beside the road for run-off (so parallel stretches don't overlap)
    let (room_r, room_l) = room_beside(&cl, opt.grass_m);

    // --- build the graph from the template
    let tpl = find_templates(template)?;
    let tpl_track = Track::from_graph(template)?;
    let mut nodes: HashMap<i32, Node> = HashMap::new();
    let mut next = template.next_id();
    // centre the track on the origin
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in &pieces {
        for q in [p.p0, p.end()] {
            for k in 0..2 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
        }
    }
    let off = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
    report.offset = off;
    // Run-off width per side at every join between pieces: the room left beside both pieces
    // that meet there (and short of a turn's centre). Within a piece the width runs straight
    // from one join's value to the next, so it never goes past the room and walls don't step.
    let np = pieces.len();
    let mut room_piece = vec![[opt.grass_m; 2]; np]; // [right, left]
    for (k, pc) in pieces.iter().enumerate() {
        let (a, b) = (sample(pc.s0), sample(pc.s0 + pc.len));
        let b_idx = if b <= a { b + n } else { b };
        let mut r = (a..=b_idx).map(|j| room_r[j % n]).fold(opt.grass_m, f64::min);
        let mut l = (a..=b_idx).map(|j| room_l[j % n]).fold(opt.grass_m, f64::min);
        if !pc.straight() {
            let radius = pc.len / (pc.h1 - pc.h0).abs();
            let half = 0.5 * prof.w[a].max(prof.w[b % n]);
            let room = (radius - half - 0.75).max(0.0);
            if pc.h1 > pc.h0 {
                l = l.min(room);
            } else {
                r = r.min(room);
            }
        }
        room_piece[k] = [r, l];
    }
    let join: Vec<[f64; 2]> = (0..np)
        .map(|k| {
            let (p, q) = (room_piece[(k + np - 1) % np], room_piece[k]);
            [p[0].min(q[0]), p[1].min(q[1])]
        })
        .collect();
    let mut seg_ids = Vec::with_capacity(pieces.len());
    for (pi, pc) in pieces.iter().enumerate() {
        let (a, b) = (sample(pc.s0), sample(pc.s0 + pc.len));
        let straight = pc.straight();
        let radius = if straight { f64::INFINITY } else { pc.len / (pc.h1 - pc.h0).abs() };
        let inside = (pc.h1 - pc.h0).signum(); // +1: centre on the left
        // road half widths at each end, and the run-off beyond (per end, see `join`)
        let half = [0.5 * prof.w[a], 0.5 * prof.w[b]];
        let (j0, j1) = (join[pi], join[(pi + 1) % np]);
        let side = |v0: f64, v1: f64| -> Option<[f64; 2]> { (v0.max(v1) > 0.05).then(|| [v0.max(0.05), v1.max(0.05)]) };
        let grass_r = side(j0[0], j1[0]);
        let grass_l = side(j0[1], j1[1]);
        let len_at = |d: f64| if straight { pc.len } else { ((radius - inside * d) * (pc.h1 - pc.h0).abs()).max(0.01) };
        let path_scale = |d: f64| len_at(d) / pc.len;
        // cross-section points: dlat per end, height per end, slope per end
        let mut xs: Vec<([f64; 2], [f64; 2], [f64; 2])> = Vec::new();
        let edge = |h: &Vec<f64>, d: [f64; 2]| -> ([f64; 2], [f64; 2], [f64; 2]) {
            // grade along this edge, kept to a believable road grade (30%)
            let m = |i: usize, dd: f64| (slope_at(h, i, ds) / path_scale(dd)).clamp(-0.3, 0.3);
            (d, [h[a], h[b]], [m(a, d[0]), m(b, d[1])])
        };
        let r_road = [-half[0], -half[1]];
        let l_road = [half[0], half[1]];
        let gr = grass_r.unwrap_or([0.0; 2]);
        let gl = grass_l.unwrap_or([0.0; 2]);
        let r_out = [-half[0] - gr[0], -half[1] - gr[1]];
        let l_out = [half[0] + gl[0], half[1] + gl[1]];
        if grass_r.is_some() {
            xs.push(edge(&prof.hr, r_out));
        }
        xs.push(edge(&prof.hr, r_road));
        xs.push(edge(&prof.hc, [0.0, 0.0]));
        xs.push(edge(&prof.hl, l_road));
        if grass_l.is_some() {
            xs.push(edge(&prof.hl, l_out));
        }
        let mut x_ids = Vec::new();
        for (d, h, m) in &xs {
            let mut x = tpl.x.clone();
            x.id = next;
            next += 1;
            for (f, v) in [("dlat0", d[0]), ("dlat1", d[1]), ("alt0", h[0]), ("alt1", h[1]), ("slope0", m[0]), ("slope1", m[1])] {
                x.set(f, json!(v))?;
            }
            x_ids.push(x.id);
            nodes.insert(x.id, x);
        }
        // ground strips, right to left: grass, asphalt, grass
        let corners = |r: [f64; 2], l: [f64; 2]| Corners {
            d: [r[0], r[1], l[1], l[0]],
            len_r: len_at(0.5 * (r[0] + r[1])),
            len_l: len_at(0.5 * (l[0] + l[1])),
        };
        let mut f_ids = Vec::new();
        let strips: Vec<(i32, [f64; 2], [f64; 2])> = {
            let mut v = Vec::new();
            if grass_r.is_some() {
                v.push((tpl.grass, r_out, r_road));
            }
            v.push((tpl.asphalt, r_road, l_road));
            if grass_l.is_some() {
                v.push((tpl.grass, l_road, l_out));
            }
            v
        };
        for (src, r, l) in strips {
            let tc = template_corners(&tpl_track, src).context("template strip geometry")?;
            let id = clone_part(template, &mut nodes, &mut next, src, tc, corners(r, l))?;
            let f = nodes.get_mut(&id).unwrap();
            f.set("dlat0", json!(r[0]))?;
            f.set("dlat1", json!(r[1]))?;
            f_ids.push(id);
        }
        // walls at the outer edges
        let mut w_ids = Vec::new();
        if opt.walls {
            let wt = template.node(tpl.wall).context("template wall")?;
            let wh = 0.5 * wt.f("width0").max(0.05);
            for d in [[r_out[0] + wh, r_out[1] + wh], [l_out[0] - wh, l_out[1] - wh]] {
                let tc = template_corners(&tpl_track, tpl.wall).context("template wall geometry")?;
                let len = len_at(0.5 * (d[0] + d[1]));
                let id = clone_part(template, &mut nodes, &mut next, tpl.wall, tc, Corners { d: [d[0], d[1], d[1], d[0]], len_r: len, len_l: len })?;
                let w = nodes.get_mut(&id).unwrap();
                w.set("dlat0", json!(d[0]))?;
                w.set("dlat1", json!(d[1]))?;
                w_ids.push(id);
            }
        }
        // the piece itself
        let mut seg = tpl.seg.clone();
        seg.id = next;
        next += 1;
        let end = pc.end();
        let start = if pi == 0 { pc.p0 } else { pieces[pi - 1].end() };
        let sx = start[0] - off[0];
        let sy = start[1] - off[1];
        let (ex, ey) = if pi + 1 == pieces.len() { (pieces[0].p0[0] - off[0], pieces[0].p0[1] - off[1]) } else { (end[0] - off[0], end[1] - off[1]) };
        seg.set("type", json!(if straight { 0 } else { 1 }))?;
        for (f, v) in [("sx", sx), ("sy", sy), ("sh", wrap_heading(pc.h0)), ("ex", ex), ("ey", ey), ("eh", wrap_heading(pc.h1))] {
            seg.set(f, json!(v))?;
        }
        let seg = seg.with_list("x", &x_ids)?.with_list("f", &f_ids)?.with_list("w", &w_ids)?;
        seg_ids.push(seg.id);
        nodes.insert(seg.id, seg);
    }
    // the root: the template's settings with the new pieces and no objects
    let mut root = template.root().clone().with_list("tsoref", &[])?;
    let at = root.fields.iter().position(|f| f.0 == "seg_size").context("template root has no piece list")?;
    root.fields.retain(|f| !f.0.starts_with("seg["));
    root.set("segment_count", json!(seg_ids.len()))?;
    root.fields[at].2 = json!(seg_ids.len() * 4);
    let list: Vec<(String, String, Value)> = seg_ids.iter().enumerate().map(|(k, id)| (format!("seg[{k}]"), "obj".to_string(), json!(id))).collect();
    root.fields.splice(at + 1..at + 1, list);
    let mut g = template.clone();
    g.nodes.extend(nodes);
    g.nodes.insert(root.id, root);
    let g = g.canonical(); // drop the template's own pieces and objects

    let t = Track::from_graph(&g)?;
    report.pieces = t.segments.len();
    report.curves = t.segments.iter().filter(|s| s.curve).count();
    report.straights = report.pieces - report.curves;
    report.length_nr = t.length;
    let problems = crate::track::check::check(&t);
    report.crashes = problems.iter().filter(|p| p.level == crate::track::check::Level::Crash).map(|p| p.text.clone()).collect();
    report.warnings = problems.len() - report.crashes.len();
    Ok(Converted { graph: g, report })
}

/// Texture files the track uses (as named in it).
pub fn texture_files(g: &Graph) -> Vec<String> {
    let mut v: Vec<String> = g
        .nodes
        .values()
        .filter(|n| n.class == "TextureDescriptor")
        .filter_map(|n| n.s("file").map(str::to_string))
        .filter(|f| !f.is_empty())
        .collect();
    v.sort_by_key(|f| f.to_ascii_lowercase());
    v.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    v
}

/// Where a converted track was written.
#[derive(Debug)]
pub struct Written {
    pub ptf: std::path::PathBuf,
    /// Textures copied from the template track (as named in the track).
    pub copied: Vec<String>,
    /// Textures the track uses that couldn't be found in the template's folder.
    pub missing: Vec<String>,
}

/// Save a converted track as `<dir>\<name>.ptf` (checked and backed up like every save) and
/// copy the textures it uses from the template track's folder into `<dir>`, keeping their
/// paths (`trackmat\...`). Existing files are never overwritten.
pub fn write_track(c: &Converted, dir: &std::path::Path, name: &str, template_dir: &std::path::Path) -> Result<Written> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let ptf = dir.join(format!("{name}.ptf"));
    crate::track::save::save_graph(&c.graph, &ptf)?;
    let store = crate::track::textures::TexStore::new(template_dir);
    let (mut copied, mut missing) = (Vec::new(), Vec::new());
    for f in texture_files(&c.graph) {
        let to = dir.join(f.replace('\\', "/"));
        if to.is_file() {
            continue;
        }
        match store.bytes(&f) {
            Some(b) => {
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
                }
                std::fs::write(&to, b).with_context(|| format!("writing {}", to.display()))?;
                copied.push(f);
            }
            None => missing.push(f),
        }
    }
    Ok(Written { ptf, copied, missing })
}
