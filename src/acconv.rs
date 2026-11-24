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
    /// Grass beside the road on each side (metres) where AC's own ground can't be read.
    pub grass_m: f64,
    /// A wall at the outer edge of the ground where AC has none, so cars can't fall off.
    pub walls: bool,
    /// Use AC's own ground (road, kerbs, grass, sand...) and walls (phase 3).
    pub ac_ground: bool,
    /// How far beside the road AC's ground is followed at most (metres).
    pub ground_m: f64,
    /// Pieces at least this long are split where AC's ground changes along them (metres).
    pub min_split_m: f64,
}

impl Options {
    /// Follows AC a little less closely, for far fewer pieces.
    pub fn fewer_pieces() -> Self {
        Self { position_m: 0.6, height_m: 0.2, width_m: 0.6, min_split_m: 30.0, ..Self::default() }
    }
}

impl Default for Options {
    fn default() -> Self {
        Self { position_m: 0.3, height_m: 0.1, width_m: 0.3, grass_m: 12.0, walls: true, ac_ground: true, ground_m: 30.0, min_split_m: 12.0 }
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
    // Open-world maps (FDR: 160 km "laps") are far beyond what an NR2003 track can be.
    let lap = ai.last().map_or(0.0, |p| p.distance as f64);
    if lap > 40_000.0 {
        bail!("this layout is {:.0} km round; NR2003 tracks can't be anywhere near that long (the longest real circuits are about 25 km)", lap / 1000.0);
    }
    let have_edges = ai.iter().filter(|p| p.left + p.right > 1.0).count() > n / 2;
    // The AI line (smoothed over ~8 m; points are 1-2 m apart), and how far the middle of the
    // road is to its left (smoothed over ~20 m: AC's edge distances can jump, and a jump would
    // put a kink in the centerline), and the width (~30 m).
    let spacing = ai.last().map_or(1.0, |p| p.distance as f64 / n as f64).max(0.3);
    let hp = ((4.0 / spacing).round() as usize).max(1);
    let hs = ((10.0 / spacing).round() as usize).max(1);
    let hw = ((15.0 / spacing).round() as usize).max(1);
    let col = |f: &dyn Fn(&AcAiPoint) -> f64| -> Vec<f64> { ai.iter().map(f).collect() };
    let px = smooth_closed(&col(&|p| p.pos[0] as f64), hp);
    let py = smooth_closed(&col(&|p| p.pos[1] as f64), hp);
    let cz = smooth_closed(&col(&|p| p.pos[2] as f64), hp);
    let edge = |p: &AcAiPoint| if have_edges { (p.left as f64, p.right as f64) } else { (6.0, 6.0) };
    let shift = smooth_closed(&col(&|p| 0.5 * (edge(p).0 - edge(p).1)), hs);
    let w = smooth_closed(&col(&|p| (edge(p).0 + edge(p).1).max(3.0)), hw);
    let mut cx = Vec::with_capacity(n);
    let mut cy = Vec::with_capacity(n);
    for k in 0..n {
        let (a, b) = ((k + n - 1) % n, (k + 1) % n);
        let (dx, dy) = (px[b] - px[a], py[b] - py[a]);
        let len = dx.hypot(dy).max(1e-9);
        let (lx, ly) = (-dy / len, dx / len); // left of travel
        cx.push(px[k] + lx * shift[k]);
        cy.push(py[k] + ly * shift[k]);
    }
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
    // direction over 6 m (3 samples either side), steadier than neighbour to neighbour
    for i in 0..=n {
        let (a, b) = (cl.pos[(i + n - 3) % n], cl.pos[(i + 3) % n]);
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

fn rot90(v: [f64; 2]) -> [f64; 2] {
    [-v[1], v[0]]
}
fn rotm90(v: [f64; 2]) -> [f64; 2] {
    [v[1], -v[0]]
}
fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn sub2(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn add2(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}
fn scale2(a: [f64; 2], k: f64) -> [f64; 2] {
    [a[0] * k, a[1] * k]
}

/// How the point `d` metres into an arc piece moves when its turn changes (per radian):
/// the integral of (u/len) * left(heading(u)) du from 0 to d (Simpson's rule).
fn turn_derivative(pc: &Piece, d: f64) -> [f64; 2] {
    if d <= 0.0 {
        return [0.0, 0.0];
    }
    let k = (pc.h1 - pc.h0) / pc.len;
    let f = |u: f64| {
        let h = pc.h0 + k * u;
        scale2([-h.sin(), h.cos()], u / pc.len)
    };
    let n = 16;
    let hstep = d / n as f64;
    let mut acc = add2(f(0.0), f(d));
    for q in 1..n {
        acc = add2(acc, scale2(f(q as f64 * hstep), if q % 2 == 1 { 4.0 } else { 2.0 }));
    }
    scale2(acc, hstep / 3.0)
}

/// Solve the symmetric positive-definite system `a x = b` (Cholesky). None if it isn't.
fn solve_spd(mut a: Vec<f64>, mut b: Vec<f64>, n: usize) -> Option<Vec<f64>> {
    for c in 0..n {
        let mut d = a[c * n + c];
        for k in 0..c {
            d -= a[c * n + k] * a[c * n + k];
        }
        if d <= 1e-14 {
            return None;
        }
        let d = d.sqrt();
        a[c * n + c] = d;
        for r in c + 1..n {
            let mut v = a[r * n + c];
            for k in 0..c {
                v -= a[r * n + k] * a[c * n + k];
            }
            a[r * n + c] = v / d;
        }
    }
    for r in 0..n {
        let mut v = b[r];
        for k in 0..r {
            v -= a[r * n + k] * b[k];
        }
        b[r] = v / a[r * n + r];
    }
    for r in (0..n).rev() {
        let mut v = b[r];
        for k in r + 1..n {
            v -= a[k * n + r] * b[k];
        }
        b[r] = v / a[r * n + r];
    }
    Some(b)
}

/// Least-squares refinement of the arcs' turns, the start heading and the start position
/// (Levenberg-Marquardt), with the lap's closing gap and total turn as heavily weighted extra
/// conditions.
///
/// Speed: turning arc k swings everything after it about the point `a_k` (its end, shifted by
/// how the end itself moves), so the Jacobian column for k at a sample P after it is
/// rot90(P - a_k). Its dot products with other columns are then |P|² - P·(a_k + a_l) + a_k·a_l,
/// whose sums over samples come from running totals: building the normal equations costs
/// O(pieces²) rather than O(pieces² × samples).
fn refine(fit: &mut Fit, cl: &Centerline, total: f64, iterations: usize) {
    let np = fit.turn.len();
    let curves: Vec<usize> = (0..np).filter(|&k| !fit.straight[k]).collect();
    if curves.is_empty() {
        return;
    }
    let nc = curves.len();
    let (hc, xc, yc) = (nc, nc + 1, nc + 2); // columns: start heading, start x, start y
    let nparam = nc + 3;
    let (w_pos, w_head) = (50.0, 2000.0);
    let step = if cl.len() > 6000 { 2 } else { 1 };
    let samples: Vec<usize> = (0..cl.len()).step_by(step).collect();

    // residual cost of the current fit
    let cost_of = |fit: &Fit| -> f64 {
        let pieces = fit.pieces(cl.ds);
        let mut c = 0.0;
        let mut k = 0;
        for &i in &samples {
            while k + 1 < pieces.len() && fit.knots[k + 1] <= i {
                k += 1;
            }
            let p = pieces[k].at((i - fit.knots[k]) as f64 * cl.ds);
            c += (p[0] - cl.pos[i][0]).powi(2) + (p[1] - cl.pos[i][1]).powi(2);
        }
        let end = pieces.last().map_or(fit.p0, |p| p.end());
        c += w_pos * w_pos * ((end[0] - fit.p0[0]).powi(2) + (end[1] - fit.p0[1]).powi(2));
        let turned: f64 = fit.turn.iter().sum();
        c += (w_head * (turned - total)).powi(2);
        c
    };

    let mut cost = cost_of(fit);
    let mut lambda = 1e-3;
    for _ in 0..iterations {
        let pieces = fit.pieces(cl.ds);
        let p0 = fit.p0;
        // per piece: end point and (for arcs) the swing point a_k
        let mut a_pt = vec![[0.0; 2]; np];
        let mut c_end = vec![[0.0; 2]; np];
        for k in 0..np {
            if !fit.straight[k] {
                c_end[k] = turn_derivative(&pieces[k], pieces[k].len);
                a_pt[k] = sub2(pieces[k].end(), rotm90(c_end[k]));
            }
        }
        // per piece totals over its samples
        let mut cnt = vec![0.0; np + 1];
        let mut s1 = vec![[0.0; 2]; np + 1];
        let mut s2 = vec![0.0; np + 1];
        let mut u1 = vec![[0.0; 2]; np + 1];
        let mut u2 = vec![0.0; np + 1];
        let mut sr = [0.0; 2];
        // samples inside an arc: how they move with that arc's own turn
        let mut t1 = vec![[0.0; 2]; np];
        let mut t2 = vec![0.0; np];
        let mut wsum = vec![[0.0; 2]; np];
        let mut ww = vec![0.0; np];
        let mut wr = vec![0.0; np];
        let mut k = 0;
        for &i in &samples {
            while k + 1 < np && fit.knots[k + 1] <= i {
                k += 1;
            }
            let d = (i - fit.knots[k]) as f64 * cl.ds;
            let pt = pieces[k].at(d);
            let r = sub2(pt, cl.pos[i]);
            cnt[k] += 1.0;
            s1[k] = add2(s1[k], pt);
            s2[k] += dot(pt, pt);
            let rr = rotm90(r);
            u1[k] = add2(u1[k], rr);
            u2[k] += dot(pt, rr);
            sr = add2(sr, r);
            if !fit.straight[k] {
                let w = turn_derivative(&pieces[k], d);
                let rw = rotm90(w);
                t1[k] = add2(t1[k], rw);
                t2[k] += dot(rw, pt);
                wsum[k] = add2(wsum[k], w);
                ww[k] += dot(w, w);
                wr[k] += dot(w, r);
            }
        }
        // suffix totals: everything from piece q on
        for q in (0..np).rev() {
            cnt[q] += cnt[q + 1];
            s1[q] = add2(s1[q], s1[q + 1]);
            s2[q] += s2[q + 1];
            u1[q] = add2(u1[q], u1[q + 1]);
            u2[q] += u2[q + 1];
        }
        let pend = pieces.last().map_or(p0, |p| p.end());
        let rc = scale2(sub2(pend, p0), w_pos);
        let turned: f64 = fit.turn.iter().sum();
        let rh = w_head * (turned - total);

        // normal equations
        let mut am = vec![0.0; nparam * nparam];
        let mut g = vec![0.0; nparam];
        let sum_pp = |m: usize, x: [f64; 2], y: [f64; 2]| s2[m] - dot(s1[m], add2(x, y)) + cnt[m] * dot(x, y);
        for (ja, &ka) in curves.iter().enumerate() {
            let aa = a_pt[ka];
            for (jb, &kb) in curves.iter().enumerate().skip(ja) {
                let ab = a_pt[kb];
                let mut v = if ka == kb {
                    sum_pp(ka + 1, aa, aa) + ww[ka]
                } else {
                    // ka < kb: samples after kb see both swings; samples inside kb see ka's swing
                    // and kb's own turn
                    sum_pp(kb + 1, aa, ab) + t2[kb] - dot(t1[kb], aa)
                };
                v += w_pos * w_pos * dot(sub2(pend, aa), sub2(pend, ab)) + w_head * w_head;
                am[ja * nparam + jb] = v;
                am[jb * nparam + ja] = v;
            }
            // with the start heading (swing about the start point)
            let v = sum_pp(ka + 1, p0, aa) + t2[ka] - dot(t1[ka], p0) + w_pos * w_pos * dot(sub2(pend, aa), sub2(pend, p0));
            am[ja * nparam + hc] = v;
            am[hc * nparam + ja] = v;
            // with the start position
            let pv = add2(rot90(sub2(s1[ka + 1], scale2(aa, cnt[ka + 1]))), wsum[ka]);
            am[ja * nparam + xc] = pv[0];
            am[xc * nparam + ja] = pv[0];
            am[ja * nparam + yc] = pv[1];
            am[yc * nparam + ja] = pv[1];
            g[ja] = u2[ka + 1] - dot(aa, u1[ka + 1]) + wr[ka] + w_pos * dot(sub2(pend, aa), rotm90(rc)) + w_head * rh;
        }
        am[hc * nparam + hc] = sum_pp(0, p0, p0) + w_pos * w_pos * dot(sub2(pend, p0), sub2(pend, p0));
        let pv = rot90(sub2(s1[0], scale2(p0, cnt[0])));
        am[hc * nparam + xc] = pv[0];
        am[xc * nparam + hc] = pv[0];
        am[hc * nparam + yc] = pv[1];
        am[yc * nparam + hc] = pv[1];
        am[xc * nparam + xc] = cnt[0];
        am[yc * nparam + yc] = cnt[0];
        g[hc] = u2[0] - dot(p0, u1[0]) + w_pos * dot(sub2(pend, p0), rotm90(rc));
        g[xc] = sr[0];
        g[yc] = sr[1];

        // damped step, retried with more damping until the cost drops
        let mut improved = false;
        for _ in 0..8 {
            let mut aug = am.clone();
            for q in 0..nparam {
                aug[q * nparam + q] += lambda * (1.0 + am[q * nparam + q]);
            }
            let Some(delta) = solve_spd(aug, g.iter().map(|x| -x).collect(), nparam) else {
                lambda *= 10.0;
                continue;
            };
            let mut trial = Fit { knots: fit.knots.clone(), straight: fit.straight.clone(), turn: fit.turn.clone(), h0: fit.h0, p0: fit.p0 };
            for (j, &k) in curves.iter().enumerate() {
                trial.turn[k] += delta[j];
            }
            trial.h0 += delta[hc];
            trial.p0 = [trial.p0[0] + delta[xc], trial.p0[1] + delta[yc]];
            let c2 = cost_of(&trial);
            if c2 < cost {
                let gain = (cost - c2) / cost.max(1e-12);
                *fit = trial;
                cost = c2;
                lambda = (lambda * 0.3).max(1e-9);
                improved = gain > 1e-7;
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    // the total turn exactly (the weighted condition leaves micro-radians): the biggest arc
    let turned: f64 = fit.turn.iter().sum();
    if let Some(&k) = curves.iter().max_by(|&&a, &&b| fit.turn[a].abs().total_cmp(&fit.turn[b].abs())) {
        fit.turn[k] += total - turned;
    }
}

/// Most pieces a conversion may start with (see fit_plan).
const MAX_PIECES: usize = 3000;

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
        bail!("this track isn't a loop (it runs from one place to another); NR2003 tracks have to go round in a lap");
    }
    // a first fit of the heading within ~0.8°, then refine and split where it strays
    let mut knots = split_knots(&h, 0.8f64.to_radians());
    // Far more pieces than any NR2003 track has (the most installed: 660) means an open-world
    // map or a very wiggly road; fitting it would take ages and the game couldn't use it.
    if knots.len() > MAX_PIECES {
        bail!(
            "this layout would need about {} pieces; that's far more than NR2003 tracks have (the biggest installed track has 660), so it isn't converted",
            knots.len()
        );
    }
    for _ in 0..40 {
        let (straight, turn) = classify(&h, &knots);
        let mut fit = Fit { knots: knots.clone(), straight, turn, h0: h[0], p0: cl.pos[0] };
        refine(&mut fit, cl, total, 30);
        let (worst, _, mean) = fit.worst(cl);
        if std::env::var_os("TTK_ACCONV_DEBUG").is_some() {
            let (_, at, _) = fit.worst(cl);
            eprintln!("[acconv] round: {} pieces, worst {worst:.2} m at {:.0} m round the lap", fit.turn.len(), at as f64 * cl.ds);
        }
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
            if *e > tol_m && b - a >= 4 {
                // split near the worst point, but not right at the ends
                add.push((*i).clamp(a + 2, b - 2));
            }
        }
        // stop splitting at the cap: a slightly looser fit beats one NR2003 can't use
        if worst <= tol_m || add.is_empty() || knots.len() + add.len() > MAX_PIECES {
            return Ok((fit.pieces(cl.ds), worst, mean));
        }
        knots.extend(add);
        knots.sort_unstable();
        knots.dedup();
    }
    let (straight, turn) = classify(&h, &knots);
    let mut fit = Fit { knots, straight, turn, h0: h[0], p0: cl.pos[0] };
    refine(&mut fit, cl, total, 40);
    let (worst, _, mean) = fit.worst(cl);
    Ok((fit.pieces(cl.ds), worst, mean))
}

/// The gap left between the last piece's end and the first piece's start. When the pieces are
/// written, every join point moves by the share of it at its distance round the lap, so the
/// lap closes exactly and a piece's direction changes by only about gap / lap length.
fn closing_gap(pieces: &[Piece]) -> [f64; 2] {
    match (pieces.first(), pieces.last()) {
        (Some(f), Some(l)) => {
            let e = l.end();
            [f.p0[0] - e[0], f.p0[1] - e[1]]
        }
        _ => [0.0, 0.0],
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
    /// AC surface (index into the scene's surfaces) of each triangle.
    surf: Vec<u16>,
    grid: HashMap<(i32, i32), Vec<u32>>,
}

impl RoadHeights {
    pub fn new(s: &AcScene) -> Self {
        let cell = 4.0;
        let mut tris = Vec::new();
        let mut surf = Vec::new();
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for m in &s.meshes {
            let Some(sf) = m.surface.and_then(|k| s.surfaces.get(k)) else { continue };
            if sf.key == "WALL" {
                continue;
            }
            let sk = m.surface.unwrap_or(0) as u16;
            for t in m.indices.chunks_exact(3) {
                let v = |i: u32| {
                    let b = i as usize * 8;
                    [m.verts[b], m.verts[b + 1], m.verts[b + 2]]
                };
                let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
                let id = tris.len() as u32;
                tris.push([a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]]);
                surf.push(sk);
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
        Self { cell, tris, surf, grid }
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// Height of the driving surface under (x, y) closest to `near`, if any.
    pub fn at(&self, x: f64, y: f64, near: f64) -> Option<f64> {
        self.at_surface(x, y, near, 2.0).map(|h| h.0)
    }

    /// Height and AC surface (index) of the driving surface under (x, y) closest to `near`,
    /// within `within` metres of it.
    pub fn at_surface(&self, x: f64, y: f64, near: f64, within: f64) -> Option<(f64, usize)> {
        let key = ((x / self.cell).floor() as i32, (y / self.cell).floor() as i32);
        let mut best: Option<(f64, usize)> = None;
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
            if best.is_none_or(|b| (z - near).abs() < (b.0 - near).abs()) {
                best = Some((z, self.surf[id as usize] as usize));
            }
        }
        best.filter(|b| (b.0 - near).abs() < within)
    }
}

/// AC's walls (meshes named <digits>WALL...), seen from above as line segments with the
/// height of their top, for finding the wall beside the road.
pub struct AcWalls {
    cell: f64,
    segs: Vec<([f64; 2], [f64; 2], f64)>,
    grid: HashMap<(i32, i32), Vec<u32>>,
}

impl AcWalls {
    pub fn new(s: &AcScene) -> Self {
        let cell = 6.0;
        let mut segs = Vec::new();
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for m in &s.meshes {
            let is_wall = m.surface.and_then(|k| s.surfaces.get(k)).is_some_and(|sf| sf.key == "WALL");
            if !is_wall {
                continue;
            }
            for t in m.indices.chunks_exact(3) {
                let v = |i: u32| {
                    let b = i as usize * 8;
                    [m.verts[b] as f64, m.verts[b + 1] as f64, m.verts[b + 2] as f64]
                };
                let p = [v(t[0]), v(t[1]), v(t[2])];
                // a wall triangle seen from above is (nearly) a line: its longest edge in x/y
                let mut best = (0.0, 0, 1);
                for (i, j) in [(0, 1), (1, 2), (2, 0)] {
                    let d = (p[i][0] - p[j][0]).hypot(p[i][1] - p[j][1]);
                    if d > best.0 {
                        best = (d, i, j);
                    }
                }
                if best.0 < 0.05 {
                    continue;
                }
                let top = p[0][2].max(p[1][2]).max(p[2][2]);
                let (a, b) = ([p[best.1][0], p[best.1][1]], [p[best.2][0], p[best.2][1]]);
                let id = segs.len() as u32;
                segs.push((a, b, top));
                let (x0, x1) = ((a[0].min(b[0]) / cell).floor() as i32, (a[0].max(b[0]) / cell).floor() as i32);
                let (y0, y1) = ((a[1].min(b[1]) / cell).floor() as i32, (a[1].max(b[1]) / cell).floor() as i32);
                if (x1 - x0) > 40 || (y1 - y0) > 40 {
                    continue;
                }
                for gx in x0..=x1 {
                    for gy in y0..=y1 {
                        grid.entry((gx, gy)).or_default().push(id);
                    }
                }
            }
        }
        Self { cell, segs, grid }
    }

    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }

    /// The first wall along the ray from `p` in direction `dir` (unit), within `max` metres:
    /// (distance, height of its top).
    pub fn cast(&self, p: [f64; 2], dir: [f64; 2], max: f64) -> Option<(f64, f64)> {
        let mut best: Option<(f64, f64)> = None;
        let steps = (max / self.cell).ceil() as usize + 1;
        let mut seen = std::collections::HashSet::new();
        for k in 0..=steps {
            let q = [p[0] + dir[0] * (k as f64 * self.cell).min(max), p[1] + dir[1] * (k as f64 * self.cell).min(max)];
            let (gx, gy) = ((q[0] / self.cell).floor() as i32, (q[1] / self.cell).floor() as i32);
            for cx in gx - 1..=gx + 1 {
                for cy in gy - 1..=gy + 1 {
                    let Some(list) = self.grid.get(&(cx, cy)) else { continue };
                    for &id in list {
                        if !seen.insert(id) {
                            continue;
                        }
                        let (a, b, top) = self.segs[id as usize];
                        // ray p + t dir against segment a + u (b - a)
                        let e = [b[0] - a[0], b[1] - a[1]];
                        let den = dir[0] * e[1] - dir[1] * e[0];
                        if den.abs() < 1e-9 {
                            continue;
                        }
                        let w = [a[0] - p[0], a[1] - p[1]];
                        let t = (w[0] * e[1] - w[1] * e[0]) / den;
                        let u = (w[0] * dir[1] - w[1] * dir[0]) / den;
                        if t > 0.2 && t <= max && (-1e-6..=1.0 + 1e-6).contains(&u) && best.is_none_or(|bb| t < bb.0) {
                            best = Some((t, top));
                        }
                    }
                }
            }
            if best.is_some_and(|bb| bb.0 < k as f64 * self.cell - self.cell) {
                break;
            }
        }
        best
    }
}

/// NR2003 ground surface for an AC surface (by its name and whether it counts as track).
pub fn nr_surface(key: &str, valid: bool) -> i64 {
    let k = key.to_ascii_uppercase();
    let has = |w: &[&str]| w.iter().any(|x| k.contains(x));
    if has(&["KERB", "CURB", "RUMBLE"]) {
        12
    } else if has(&["SAND", "TRAP"]) {
        6
    } else if has(&["GRAVEL"]) {
        7
    } else if has(&["DIRT", "MUD"]) {
        5
    } else if has(&["GRAS", "GRS", "GREEN"]) {
        4
    } else if has(&["CONCRETE", "PIT", "SIDEWALK", "CEMENT"]) {
        2
    } else if valid {
        1
    } else {
        4
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
    /// Gap the fit left at the start/finish line, spread round the lap when writing (metres).
    pub closing_gap_m: f64,
    /// AI points whose track edges were missing from AC's file and were measured or filled in.
    pub edges_measured: usize,
    /// Pieces whose ground and walls came from AC (the rest: plain asphalt and grass).
    pub ac_ground_pieces: usize,
    /// Walls placed where AC has walls, and safety walls at the edge of the ground.
    pub ac_walls: usize,
    pub safety_walls: usize,
    /// Ground strips by NR2003 surface number.
    pub strips_by_surface: Vec<(i64, usize)>,
    pub warnings: usize,
}

/// A converted track: the .ptf graph and the numbers.
pub struct Converted {
    pub graph: Graph,
    pub report: Report,
}

/// Template nodes to copy pieces from (found in a stock track such as watkins_glen or hillside).
struct Templates {
    seg: Node,
    x: Node,
    asphalt: i32,
    grass: i32,
    wall: i32,
    /// The template strip used for each NR2003 surface (the most common textured one).
    by_surface: HashMap<i64, i32>,
}

impl Templates {
    /// Template strip for a surface, falling back to a similar one the template has.
    fn strip_for(&self, surface: i64) -> i32 {
        let order: &[i64] = match surface {
            12 => &[12, 3, 2, 1],
            6 => &[6, 7, 5, 3, 4],
            7 => &[7, 6, 5, 3, 4],
            5 => &[5, 3, 6, 4],
            2 => &[2, 1],
            4 => &[4, 5],
            _ => &[1],
        };
        let found = order.iter().find_map(|s| self.by_surface.get(s).copied());
        found.unwrap_or(if surface == 4 { self.grass } else { self.asphalt })
    }
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
    // per surface: the most common single-texture strip
    let mut counts: HashMap<(i64, String), (usize, i32)> = HashMap::new();
    for n in ids.iter().filter_map(|id| g.node(*id)) {
        if n.class != "F_SectionDescriptor" || n.get("coords").is_none() {
            continue;
        }
        if let Some(t) = tex_of(n) {
            let e = counts.entry((n.i("i15"), t)).or_insert((0, n.id));
            e.0 += 1;
        }
    }
    let mut by_surface: HashMap<i64, (usize, i32)> = HashMap::new();
    for ((surface, _), (count, id)) in counts {
        let e = by_surface.entry(surface).or_insert((0, id));
        if count > e.0 || (count == e.0 && id < e.1) {
            *e = (count, id);
        }
    }
    let mut by_surface: HashMap<i64, i32> = by_surface.into_iter().map(|(k, v)| (k, v.1)).collect();
    by_surface.insert(1, asphalt);
    if grass != asphalt {
        by_surface.insert(4, grass);
    }
    Ok(Templates { seg, x, asphalt, grass, wall, by_surface })
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

// ------------------------------------------------------------------ a piece's cross-section

/// Everything across one piece: cross-section points, ground strips and walls (right to left).
struct Layout {
    /// (dlat, height, slope) at the start and end of the piece.
    xs: Vec<([f64; 2], [f64; 2], [f64; 2])>,
    /// (NR2003 surface, right edge dlat at start and end). Each strip runs left to the next one,
    /// the last to the leftmost cross-section point.
    strips: Vec<(i64, [f64; 2])>,
    /// (centre dlat at start and end, height, from AC).
    walls: Vec<([f64; 2], f64, bool)>,
    from_ac: bool,
}

/// What's across the road at one spot: ground samples (dlat, AC height, NR2003 surface) and
/// the wall found on each side (distance from the centre, AC height of its top).
struct Cross {
    samples: Vec<(f64, f64, i64)>,
    wall: [Option<(f64, f64)>; 2],
}

/// Look across the road at `t` (0..1) along piece `pc`: step sideways from the centre on both
/// sides reading AC's ground, until it runs out, an AC wall, or `limit` (per side).
#[allow(clippy::too_many_arguments)]
fn look_across(pc: &Piece, t: f64, limit: [f64; 2], z_centre: f64, roads: &RoadHeights, walls: &AcWalls, nr: &[i64], step: f64) -> Cross {
    let d = pc.len * t;
    let p = pc.at(d);
    let h = pc.h0 + (pc.h1 - pc.h0) * t;
    let left = [-h.sin(), h.cos()];
    let mut out = Cross { samples: Vec::new(), wall: [None, None] };
    for (side, sign) in [(0usize, -1.0f64), (1, 1.0)] {
        let dir = [left[0] * sign, left[1] * sign];
        let mut lim = limit[side];
        if let Some((dist, top)) = walls.cast(p, dir, lim + 0.5) {
            out.wall[side] = Some((dist, top));
            lim = lim.min(dist);
        }
        let mut z = z_centre;
        let mut misses = 0;
        let first = if side == 0 { 1 } else { 0 };
        let mut k = first;
        loop {
            let dd = k as f64 * step;
            if dd > lim {
                break;
            }
            k += 1;
            let q = [p[0] + dir[0] * dd, p[1] + dir[1] * dd];
            match roads.at_surface(q[0], q[1], z, 0.6) {
                Some((zz, sf)) => {
                    misses = 0;
                    z = zz;
                    out.samples.push((sign * dd, zz, nr.get(sf).copied().unwrap_or(1)));
                }
                None => {
                    misses += 1;
                    if misses as f64 * step > 1.5 {
                        break;
                    }
                }
            }
        }
    }
    out.samples.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// Runs of the same surface across the road: (surface, from dlat, to dlat), right to left.
/// Strips narrower than 0.5 m are merged into a neighbour. None if there's no ground under the
/// centre of the road.
fn runs(c: &Cross) -> Option<Vec<(i64, f64, f64)>> {
    let sm = &c.samples;
    if sm.len() < 2 || !(sm[0].0 <= 0.0 && sm[sm.len() - 1].0 >= 0.0) {
        return None;
    }
    let mut out: Vec<(i64, f64, f64)> = Vec::new();
    for k in 0..sm.len() {
        let (d, _, surf) = sm[k];
        match out.last_mut() {
            Some(last) if last.0 == surf => last.2 = d,
            Some(last) => {
                let mid = 0.5 * (last.2 + d);
                last.2 = mid;
                out.push((surf, mid, d));
            }
            None => out.push((surf, d, d)),
        }
    }
    // merge narrow strips
    loop {
        let narrow = out
            .iter()
            .enumerate()
            .filter(|(_, r)| r.2 - r.1 < 0.5)
            .min_by(|a, b| (a.1 .2 - a.1 .1).total_cmp(&(b.1 .2 - b.1 .1)))
            .map(|(i, _)| i);
        let Some(i) = narrow else { break };
        if out.len() == 1 {
            break;
        }
        let into = if i == 0 {
            1
        } else if i + 1 == out.len() {
            i - 1
        } else if out[i - 1].2 - out[i - 1].1 >= out[i + 1].2 - out[i + 1].1 {
            i - 1
        } else {
            i + 1
        };
        let (a, b) = (out[i].1.min(out[into].1), out[i].2.max(out[into].2));
        out[into].1 = a;
        out[into].2 = b;
        out.remove(i);
        // join neighbours that now have the same surface
        let mut k = 0;
        while k + 1 < out.len() {
            if out[k].0 == out[k + 1].0 {
                out[k].2 = out[k + 1].2;
                out.remove(k + 1);
            } else {
                k += 1;
            }
        }
    }
    Some(out)
}

fn surfaces_of(r: &Option<Vec<(i64, f64, f64)>>) -> Option<Vec<i64>> {
    r.as_ref().map(|v| v.iter().map(|x| x.0).collect())
}

/// Keep everything on the inside of a turn (`inside` = +1 left, -1 right) within `limit` of the
/// centerline: cross-section points and strip edges past it are pulled back to it, strips left
/// with no width are dropped, and walls are moved in.
fn clip_inside(l: &mut Layout, inside: f64, limit: f64, wall_half: f64) {
    let limit = limit.max(0.1);
    let clip = |d: f64| if inside > 0.0 { d.min(limit) } else { d.max(-limit) };
    let beyond = |d: f64| if inside > 0.0 { d >= limit - 1e-9 } else { d <= -limit + 1e-9 };
    let wall_beyond = |d: f64| if inside > 0.0 { d > limit - wall_half } else { d < -limit + wall_half };
    if !l.xs.iter().any(|x| beyond(x.0[0]) || beyond(x.0[1]))
        && !l.strips.iter().any(|st| beyond(st.1[0]) || beyond(st.1[1]))
        && !l.walls.iter().any(|w| wall_beyond(w.0[0]) || wall_beyond(w.0[1]))
    {
        return;
    }
    for x in &mut l.xs {
        x.0 = [clip(x.0[0]), clip(x.0[1])];
    }
    // cross-section points pulled onto the limit at both ends: keep only the innermost of them
    let mut seen_edge = false;
    if inside > 0.0 {
        l.xs.retain(|x| {
            let at = beyond(x.0[0]) && beyond(x.0[1]);
            let keep = !at || !seen_edge;
            seen_edge |= at;
            keep
        });
    } else {
        l.xs.reverse();
        l.xs.retain(|x| {
            let at = beyond(x.0[0]) && beyond(x.0[1]);
            let keep = !at || !seen_edge;
            seen_edge |= at;
            keep
        });
        l.xs.reverse();
    }
    // keep the points in order with a little room between them
    for k in 1..l.xs.len() {
        for e in 0..2 {
            if l.xs[k].0[e] < l.xs[k - 1].0[e] + 0.05 {
                l.xs[k].0[e] = l.xs[k - 1].0[e] + 0.05;
            }
        }
    }
    let first = l.xs.first().map_or([0.0; 2], |x| x.0);
    let last = l.xs.last().map_or([0.0; 2], |x| x.0);
    for st in &mut l.strips {
        st.1 = [clip(st.1[0]).max(first[0]).min(last[0]), clip(st.1[1]).max(first[1]).min(last[1])];
    }
    // strips with no width left (the next one starts where they do, at both ends)
    let mut k = 0;
    while k < l.strips.len() {
        let next = l.strips.get(k + 1).map_or(last, |s| s.1);
        let empty = next[0] - l.strips[k].1[0] < 0.02 && next[1] - l.strips[k].1[1] < 0.02;
        if empty && l.strips.len() > 1 {
            if k == 0 {
                // the first strip starts at the right edge; the next one takes over from there
                let start = l.strips[0].1;
                l.strips.remove(0);
                l.strips[0].1 = start;
            } else {
                l.strips.remove(k);
            }
        } else {
            k += 1;
        }
    }
    if let Some(f) = l.strips.first_mut() {
        f.1 = first;
    }
    for w in &mut l.walls {
        let pull = |d: f64| if inside > 0.0 { d.min(limit - wall_half) } else { d.max(-limit + wall_half) };
        w.0 = [pull(w.0[0]), pull(w.0[1])];
    }
}

/// AC's AI points with the track edges filled in where the AI file has none (left + right = 0):
/// measured by stepping sideways over AC's track surfaces (counting as track, not kerbs or
/// grass), else interpolated from the nearest points that have them. Returns the points and
/// how many were filled.
fn fill_edges(scene: &AcScene, roads: &RoadHeights) -> (Vec<AcAiPoint>, usize) {
    let mut ai = scene.ai.clone();
    let n = ai.len();
    // missing or not believable: an edge at 0 (one-sided data) or wider than 30 m
    let bad = |p: &AcAiPoint| p.left <= 0.1 || p.right <= 0.1 || p.left > 30.0 || p.right > 30.0;
    let missing: Vec<bool> = ai.iter().map(bad).collect();
    let count = missing.iter().filter(|&&m| m).count();
    if count == 0 || n < 3 {
        return (ai, 0);
    }
    let road: Vec<bool> = scene.surfaces.iter().map(|sf| sf.valid && matches!(nr_surface(&sf.key, sf.valid), 1 | 2)).collect();
    for k in 0..n {
        if !missing[k] {
            continue;
        }
        let (a, b) = (ai[(k + n - 1) % n].pos, ai[(k + 1) % n].pos);
        let (dx, dy) = ((b[0] - a[0]) as f64, (b[1] - a[1]) as f64);
        let len = dx.hypot(dy).max(1e-9);
        let left = [-dy / len, dx / len];
        let p = ai[k].pos;
        let mut edge = [0.0f64; 2];
        for (side, sign) in [(0usize, 1.0f64), (1, -1.0)] {
            let mut z = p[2] as f64;
            let mut d = 0.0;
            let mut last = None;
            let mut misses = 0;
            while d < 40.0 {
                let q = [p[0] as f64 + left[0] * sign * d, p[1] as f64 + left[1] * sign * d];
                match roads.at_surface(q[0], q[1], z, 1.0) {
                    Some((zz, sf)) if road.get(sf).copied().unwrap_or(false) => {
                        z = zz;
                        last = Some(d);
                        misses = 0;
                    }
                    _ => {
                        misses += 1;
                        if misses > 2 && last.is_some() {
                            break;
                        }
                    }
                }
                d += 0.25;
            }
            edge[side] = last.unwrap_or(0.0);
        }
        if edge[0] > 0.5 && edge[1] > 0.5 {
            ai[k].left = edge[0] as f32;
            ai[k].right = edge[1] as f32;
        } else {
            ai[k].left = 0.0;
            ai[k].right = 0.0;
        }
    }
    // still missing: interpolate between the nearest points that have edges
    let known: Vec<usize> = (0..n).filter(|&k| !bad(&ai[k])).collect();
    if !known.is_empty() {
        for k in 0..n {
            if !bad(&ai[k]) {
                continue;
            }
            let after = known.iter().copied().find(|&j| j > k).unwrap_or(known[0] + n);
            let before = known.iter().rev().copied().find(|&j| j < k).map_or(known[known.len() - 1] as isize - n as isize, |j| j as isize);
            let span = (after as isize - before) as f32;
            let f = (k as isize - before) as f32 / span.max(1.0);
            let (pa, pb) = (ai[before.rem_euclid(n as isize) as usize], ai[after % n]);
            ai[k].left = pa.left + (pb.left - pa.left) * f;
            ai[k].right = pa.right + (pb.right - pa.right) * f;
        }
    }
    (ai, count)
}

/// Convert an AC scene into an NR2003 track graph, using `template` (a stock .ptf graph) for
/// the pieces' strips, walls and track settings.
pub fn convert(scene: &AcScene, template: &Graph, opt: &Options) -> Result<Converted> {
    let mut report = Report::default();
    // TTK_ACCONV_DEBUG=1 prints how long each stage takes
    let debug = std::env::var_os("TTK_ACCONV_DEBUG").is_some();
    let t0 = std::time::Instant::now();
    let stage = |what: &str| {
        if debug {
            eprintln!("[acconv] {what}: {:.2}s", t0.elapsed().as_secs_f64());
        }
    };
    // AC's AI file sometimes lacks the track edges for parts of the lap (ac_circuit_del_plan:
    // 40%); there they're measured on AC's track surfaces, or interpolated
    let roads = RoadHeights::new(scene);
    let (ai, filled) = fill_edges(scene, &roads);
    report.edges_measured = filled;
    let cl = centerline(&ai, 1.0)?;
    report.length_ac = cl.length();
    let (mut pieces, worst, mean) = fit_plan(&cl, opt.position_m)?;
    report.worst_m = worst;
    report.mean_m = mean;
    stage(&format!("plan fit, {} pieces, worst {worst:.2} m", pieces.len()));

    // --- heights and widths along the fitted track, every metre
    stage(&format!("road surface grid: {} triangles", roads.tris.len()));
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
    stage("road heights sampled");
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
        let b_idx = a + ((pc.len / ds).round() as usize).max(1);
        let _ = b;
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
        let over = (pc.h1 - pc.h0).abs() > 89f64.to_radians() && pc.len > 2.0 * ds;
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
        let b_idx = a + ((pc.len / ds).round() as usize).max(1);
        let _ = b;
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

    stage(&format!("heights, {} pieces", pieces.len()));
    // --- room beside the road (so parallel stretches don't overlap)
    let max_side = if opt.ac_ground { opt.ground_m.max(opt.grass_m) } else { opt.grass_m };
    let (room_r, room_l) = room_beside(&cl, max_side);
    let walls_ac = AcWalls::new(scene);
    let nr: Vec<i64> = scene.surfaces.iter().map(|sf| nr_surface(&sf.key, sf.valid)).collect();
    let use_ac = opt.ac_ground && report.heights_found > 0.5;
    // Ground that can't be read between the road and a wall: grass, or concrete on tracks with
    // no grass at all (street circuits like Monaco)
    let has_grass = scene.meshes.iter().any(|m| {
        m.surface.and_then(|k| scene.surfaces.get(k)).is_some_and(|sf| {
            let k = sf.key.to_ascii_uppercase();
            ["GRAS", "GRS", "GREEN"].iter().any(|w| k.contains(w))
        })
    });
    let filler: i64 = if has_grass { 4 } else { 2 };
    let step = 0.25;
    // AC height of the road centre at sample i (the profile is shifted by `base`)
    let z_at = |i: usize| prof.hc[i % n] + base;
    let limit_at = |i: usize| [0.5 * prof.w[i % n] + room_r[i % n], 0.5 * prof.w[i % n] + room_l[i % n]];

    // --- split pieces whose ground changes along them (a kerb starting part-way, say)
    if use_ac {
        let mut i = 0;
        while i < pieces.len() {
            let pc = pieces[i].clone();
            if pc.len >= opt.min_split_m {
                let seq = |t: f64| {
                    let j = sample(pc.s0 + pc.len * t);
                    surfaces_of(&runs(&look_across(&pc, t, limit_at(j), z_at(j), &roads, &walls_ac, &nr, step)))
                };
                let (a, m, b) = (seq(0.0), seq(0.5), seq(1.0));
                if a != m || m != b {
                    let d = (pc.len * 0.5 / ds).round() * ds;
                    let mid_h = pc.h0 + (pc.h1 - pc.h0) * d / pc.len;
                    let first = Piece { s0: pc.s0, len: d, h0: pc.h0, h1: mid_h, p0: pc.p0 };
                    let second = Piece { s0: pc.s0 + d, len: pc.len - d, h0: mid_h, h1: pc.h1, p0: first.end() };
                    pieces[i] = first;
                    pieces.insert(i + 1, second);
                    continue;
                }
            }
            i += 1;
        }
    }

    stage(&format!("ground layout splits, {} pieces", pieces.len()));
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
    let gap = closing_gap(&pieces);
    let lap: f64 = pieces.iter().map(|p| p.len).sum::<f64>().max(1.0);
    report.closing_gap_m = gap[0].hypot(gap[1]);
    // Room beside the road per side at every join between pieces: the room left beside both
    // pieces that meet there (and short of a turn's centre). Within a piece it runs straight from
    // one join's value to the next, so it never goes past the room and walls don't step.
    let np = pieces.len();
    let mut room_piece = vec![[max_side; 2]; np]; // [right, left]
    for (k, pc) in pieces.iter().enumerate() {
        let (a, b) = (sample(pc.s0), sample(pc.s0 + pc.len));
        let b_idx = a + ((pc.len / ds).round() as usize).max(1);
        let _ = b;
        let mut r = (a..=b_idx).map(|j| room_r[j % n]).fold(max_side, f64::min);
        let mut l = (a..=b_idx).map(|j| room_l[j % n]).fold(max_side, f64::min);
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
    let wall_tpl = template.node(tpl.wall).context("template wall")?.clone();
    let wh = 0.5 * wall_tpl.f("width0").max(0.05);
    let mut by_surface: HashMap<i64, usize> = HashMap::new();
    let mut seg_ids = Vec::with_capacity(pieces.len());
    for (pi, pc) in pieces.iter().enumerate() {
        let (a, b) = (sample(pc.s0), sample(pc.s0 + pc.len));
        let straight = pc.straight();
        let radius = if straight { f64::INFINITY } else { pc.len / (pc.h1 - pc.h0).abs() };
        let inside = (pc.h1 - pc.h0).signum(); // +1: centre on the left
        let len_at = |d: f64| if straight { pc.len } else { ((radius - inside * d) * (pc.h1 - pc.h0).abs()).max(0.01) };
        let path_scale = |d: f64| len_at(d) / pc.len;
        let half = [0.5 * prof.w[a], 0.5 * prof.w[b]];
        let (j0, j1) = (join[pi], join[(pi + 1) % np]);
        // grade along the road centre (from the smoothed profile), per end
        let centre_slope = |i: usize| slope_at(&prof.hc, i, ds).clamp(-0.3, 0.3);

        // ---- the layout: from AC's ground, else plain asphalt and grass
        let mut layout: Option<Layout> = None;
        // the ground's outer edges at each end as read from AC (for the fallback below, so it
        // meets AC-ground neighbours exactly): what's found, out past any AC wall
        let mut ac_edges: [Option<[f64; 2]>; 2] = [None, None];
        if use_ac {
            let lim0 = [half[0] + j0[0], half[0] + j0[1]];
            let lim1 = [half[1] + j1[0], half[1] + j1[1]];
            let c0 = look_across(pc, 0.0, lim0, z_at(a), &roads, &walls_ac, &nr, step);
            let c1 = look_across(pc, 1.0, lim1, z_at(b), &roads, &walls_ac, &nr, step);
            let cm = look_across(pc, 0.5, [0.5 * (lim0[0] + lim1[0]), 0.5 * (lim0[1] + lim1[1])], z_at(sample(pc.s0 + 0.5 * pc.len)), &roads, &walls_ac, &nr, step);
            let edges_at = |c: &Cross, r: &Vec<(i64, f64, f64)>| -> [f64; 2] {
                let mut e = [r[0].1, r.last().unwrap().2];
                for (side, sign) in [(0usize, -1.0f64), (1, 1.0)] {
                    if let Some(w) = c.wall[side] {
                        let reach = sign * (w.0 + 2.0 * wh + 0.1);
                        if (reach - e[side]) * sign > 0.0 {
                            e[side] = reach;
                        }
                    }
                }
                e
            };
            let (r0, r1) = (runs(&c0), runs(&c1));
            ac_edges = [r0.as_ref().map(|r| edges_at(&c0, r)), r1.as_ref().map(|r| edges_at(&c1, r))];
            if let (Some(r0), Some(r1)) = (r0, r1) {
                // the middle's strips (the start's where the middle can't be read)
                let rm = runs(&cm).unwrap_or_else(|| r0.clone());
                let same = |x: &Vec<(i64, f64, f64)>| x.iter().map(|q| q.0).eq(rm.iter().map(|q| q.0));
                // boundaries per end (the middle's where the ends disagree, on short pieces)
                // Outer edges at each end are what's found right there (so neighbouring pieces
                // agree at the join). Strip edges: each end's own where both ends have the same
                // strips as the middle; else the middle's, scaled between the ends' outer edges.
                let mut outer_r = [r0[0].1, r1[0].1];
                let mut outer_l = [r0.last().unwrap().2, r1.last().unwrap().2];
                let mut strips: Vec<(i64, [f64; 2])> = if same(&r0) && same(&r1) {
                    r0.iter().zip(&r1).map(|(p, q)| (p.0, [p.1, q.1])).collect()
                } else {
                    let (mr, ml) = (rm[0].1, rm.last().unwrap().2);
                    rm.iter()
                        .map(|run| {
                            let f = ((run.1 - mr) / (ml - mr).max(1e-6)).clamp(0.0, 1.0);
                            (run.0, [outer_r[0] + f * (outer_l[0] - outer_r[0]), outer_r[1] + f * (outer_l[1] - outer_r[1])])
                        })
                        .collect()
                };
                // The ground reaches past AC's walls, end by end; where AC has no readable ground
                // before a wall, the gap becomes grass. AC's wall is used where it's at both ends,
                // else a safety wall goes at the ground's edge.
                let mut walls = Vec::new();
                for (side, sign) in [(0usize, -1.0f64), (1, 1.0)] {
                    let ends = [c0.wall[side], c1.wall[side]];
                    let edge = if side == 0 { outer_r } else { outer_l };
                    let reach = |e: usize| ends[e].map(|w| sign * (w.0 + 2.0 * wh + 0.1));
                    let gap: Vec<f64> = (0..2).map(|e| reach(e).map_or(0.0, |r| ((r - edge[e]) * sign).max(0.0))).collect();
                    if gap[0].max(gap[1]) > 0.5 {
                        let g = |e: usize| edge[e] + sign * gap[e].max(0.05);
                        if side == 0 {
                            outer_r = [g(0), g(1)];
                            strips.insert(0, (filler, outer_r));
                        } else {
                            strips.push((filler, outer_l));
                            outer_l = [g(0), g(1)];
                        }
                    } else if side == 0 {
                        outer_r = [edge[0] - gap[0], edge[1] - gap[1]];
                        strips[0].1 = outer_r;
                    } else {
                        outer_l = [edge[0] + gap[0], edge[1] + gap[1]];
                    }
                    let edge = if side == 0 { outer_r } else { outer_l };
                    match (ends[0], ends[1]) {
                        (Some(w0), Some(w1)) => {
                            let d = [sign * (w0.0 + wh), sign * (w1.0 + wh)];
                            let ground = |i: usize, ww: (f64, f64)| ww.1 - z_at(i);
                            let height = (0.5 * (ground(a, w0) + ground(b, w1))).clamp(0.5, 8.0);
                            walls.push((d, height, true));
                        }
                        _ if opt.walls => walls.push(([edge[0] - sign * wh, edge[1] - sign * wh], wall_tpl.f("height0"), false)),
                        _ => {}
                    }
                }
                // cross-section points: every strip edge, the outer edges, and the road centre
                let mut ds_pts: Vec<[f64; 2]> = vec![outer_r];
                for st in strips.iter().skip(1) {
                    ds_pts.push(st.1);
                }
                ds_pts.push(outer_l);
                if !ds_pts.iter().any(|d| d[0].abs() < 0.3 && d[1].abs() < 0.3) {
                    let at = ds_pts.iter().position(|d| d[0] + d[1] > 0.0).unwrap_or(ds_pts.len());
                    ds_pts.insert(at, [0.0, 0.0]);
                }
                // keep them in order at both ends
                for k in 1..ds_pts.len() {
                    for e in 0..2 {
                        if ds_pts[k][e] < ds_pts[k - 1][e] + 0.05 {
                            ds_pts[k][e] = ds_pts[k - 1][e] + 0.05;
                        }
                    }
                }
                let delta = 3.0 / pc.len;
                let height_at = |t: f64, d: f64, near: f64| -> Option<f64> {
                    let p = pc.at(pc.len * t);
                    let h = pc.h0 + (pc.h1 - pc.h0) * t;
                    roads.at_surface(p[0] - h.sin() * d, p[1] + h.cos() * d, near, 2.5).map(|x| x.0)
                };
                let mut xs = Vec::new();
                for d in &ds_pts {
                    if d[0] == 0.0 && d[1] == 0.0 {
                        xs.push(([0.0, 0.0], [prof.hc[a], prof.hc[b % n]], [centre_slope(a) / path_scale(0.0), centre_slope(b % n) / path_scale(0.0)]));
                        continue;
                    }
                    let (za, zb) = (z_at(a), z_at(b));
                    let h0 = height_at(0.0, d[0], za).unwrap_or(za);
                    let h1 = height_at(1.0, d[1], zb).unwrap_or(zb);
                    let dm = |t: f64| d[0] + (d[1] - d[0]) * t;
                    // grade at each end measured 3 m either side of the join (the same for the
                    // pieces on both sides, so the ground has no crease there)
                    let grade = |t: f64, d: f64, h: f64, fallback: f64| -> f64 {
                        match (height_at(t - delta, d, h), height_at(t + delta, d, h)) {
                            (Some(p), Some(q)) => (q - p) / (2.0 * delta * len_at(d)),
                            _ => fallback,
                        }
                    };
                    let s0 = grade(0.0, dm(0.0), h0, centre_slope(a));
                    let s1 = grade(1.0, dm(1.0), h1, centre_slope(b % n));
                    xs.push((*d, [h0 - base, h1 - base], [s0.clamp(-0.3, 0.3), s1.clamp(-0.3, 0.3)]));
                }
                strips[0].1 = ds_pts[0];
                layout = Some(Layout { xs, strips, walls, from_ac: true });
            }
        }
        let layout = match layout {
            Some(l) => l,
            None => {
                // plain asphalt with grass beside it (where AC's ground couldn't be read)
                let side = |v0: f64, v1: f64| -> Option<[f64; 2]> { (v0.max(v1) > 0.05).then(|| [v0.max(0.05).min(opt.grass_m), v1.max(0.05).min(opt.grass_m)]) };
                let grass_r = side(j0[0], j1[0]);
                let grass_l = side(j0[1], j1[1]);
                let edge = |h: &Vec<f64>, d: [f64; 2]| -> ([f64; 2], [f64; 2], [f64; 2]) {
                    let m = |i: usize, dd: f64| (slope_at(h, i, ds) / path_scale(dd)).clamp(-0.3, 0.3);
                    (d, [h[a], h[b]], [m(a, d[0]), m(b, d[1])])
                };
                let r_road = [-half[0], -half[1]];
                let l_road = [half[0], half[1]];
                let gr = grass_r.unwrap_or([0.0; 2]);
                let gl = grass_l.unwrap_or([0.0; 2]);
                let mut r_out = [-half[0] - gr[0], -half[1] - gr[1]];
                let mut l_out = [half[0] + gl[0], half[1] + gl[1]];
                // where an end can be read from AC, its outer edges are AC's
                for e in 0..2 {
                    if let Some(edges) = ac_edges[e] {
                        r_out[e] = edges[0].min(-half[e] - 0.05);
                        l_out[e] = edges[1].max(half[e] + 0.05);
                    }
                }
                let grass_r = ((-half[0] - r_out[0]).max(-half[1] - r_out[1]) > 0.04).then_some(());
                let grass_l = ((l_out[0] - half[0]).max(l_out[1] - half[1]) > 0.04).then_some(());
                if grass_r.is_none() {
                    r_out = [-half[0], -half[1]];
                }
                if grass_l.is_none() {
                    l_out = [half[0], half[1]];
                }
                let mut xs = Vec::new();
                let mut strips = Vec::new();
                if grass_r.is_some() {
                    xs.push(edge(&prof.hr, r_out));
                    strips.push((filler, r_out));
                }
                xs.push(edge(&prof.hr, r_road));
                xs.push(edge(&prof.hc, [0.0, 0.0]));
                xs.push(edge(&prof.hl, l_road));
                strips.push((1, r_road));
                if grass_l.is_some() {
                    xs.push(edge(&prof.hl, l_out));
                    strips.push((filler, l_road));
                }
                let mut walls = Vec::new();
                if opt.walls {
                    walls.push(([r_out[0] + wh, r_out[1] + wh], wall_tpl.f("height0"), false));
                    walls.push(([l_out[0] - wh, l_out[1] - wh], wall_tpl.f("height0"), false));
                }
                Layout { xs, strips, walls, from_ac: false }
            }
        };
        // Nothing may reach past a turn's centre (Sandbox won't load it): on tight turns,
        // clip the inside short of the centre
        let mut layout = layout;
        if !straight {
            clip_inside(&mut layout, inside, radius - 0.5, wh);
        }
        if layout.from_ac {
            report.ac_ground_pieces += 1;
        }

        // ---- nodes: cross-section points
        let mut x_ids = Vec::new();
        for (d, h, m) in &layout.xs {
            let mut x = tpl.x.clone();
            x.id = next;
            next += 1;
            for (f, v) in [("dlat0", d[0]), ("dlat1", d[1]), ("alt0", h[0]), ("alt1", h[1]), ("slope0", m[0]), ("slope1", m[1])] {
                x.set(f, json!(v))?;
            }
            x_ids.push(x.id);
            nodes.insert(x.id, x);
        }
        // ground strips, right to left
        let outer_left = layout.xs.last().map_or([0.0; 2], |x| x.0);
        let mut f_ids = Vec::new();
        for (k, (surface, r)) in layout.strips.iter().enumerate() {
            let l = layout.strips.get(k + 1).map_or(outer_left, |s| s.1);
            let src = tpl.strip_for(*surface);
            let tc = template_corners(&tpl_track, src).context("template strip geometry")?;
            let corners = Corners { d: [r[0], r[1], l[1], l[0]], len_r: len_at(0.5 * (r[0] + r[1])), len_l: len_at(0.5 * (l[0] + l[1])) };
            let id = clone_part(template, &mut nodes, &mut next, src, tc, corners)?;
            let f = nodes.get_mut(&id).unwrap();
            f.set("dlat0", json!(r[0]))?;
            f.set("dlat1", json!(r[1]))?;
            // the surface cars feel is AC's, whatever texture the template strip has
            f.set("i15", json!(*surface))?;
            *by_surface.entry(*surface).or_default() += 1;
            f_ids.push(id);
        }
        // walls
        let mut w_ids = Vec::new();
        for (d, height, from_ac) in &layout.walls {
            let tc = template_corners(&tpl_track, tpl.wall).context("template wall geometry")?;
            let len = len_at(0.5 * (d[0] + d[1]));
            let id = clone_part(template, &mut nodes, &mut next, tpl.wall, tc, Corners { d: [d[0], d[1], d[1], d[0]], len_r: len, len_l: len })?;
            let w = nodes.get_mut(&id).unwrap();
            w.set("dlat0", json!(d[0]))?;
            w.set("dlat1", json!(d[1]))?;
            w.set("height0", json!(*height))?;
            w.set("height1", json!(*height))?;
            if *from_ac {
                report.ac_walls += 1;
            } else {
                report.safety_walls += 1;
            }
            w_ids.push(id);
        }
        // the piece itself
        let mut seg = tpl.seg.clone();
        seg.id = next;
        next += 1;
        let end = pc.end();
        // join points moved by their share of the closing gap (see `closing_gap`)
        let share = |s: f64| [gap[0] * s / lap, gap[1] * s / lap];
        let (f0, f1) = (share(pc.s0), share(pc.s0 + pc.len));
        let sx = pc.p0[0] + f0[0] - off[0];
        let sy = pc.p0[1] + f0[1] - off[1];
        let (ex, ey) = if pi + 1 == pieces.len() {
            (pieces[0].p0[0] - off[0], pieces[0].p0[1] - off[1])
        } else {
            (end[0] + f1[0] - off[0], end[1] + f1[1] - off[1])
        };
        seg.set("type", json!(if straight { 0 } else { 1 }))?;
        for (f, v) in [("sx", sx), ("sy", sy), ("sh", wrap_heading(pc.h0)), ("ex", ex), ("ey", ey), ("eh", wrap_heading(pc.h1))] {
            seg.set(f, json!(v))?;
        }
        let seg = seg.with_list("x", &x_ids)?.with_list("f", &f_ids)?.with_list("w", &w_ids)?;
        seg_ids.push(seg.id);
        nodes.insert(seg.id, seg);
    }
    let mut sv: Vec<(i64, usize)> = by_surface.into_iter().collect();
    sv.sort_unstable();
    report.strips_by_surface = sv;
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
