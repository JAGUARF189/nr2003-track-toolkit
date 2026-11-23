//! Track checker: finds what would stop a track loading ("crash") or look wrong ("warning").
//!
//! The crash rules come from Sandbox's own loading checks (sandbox.exe; see docs/ptf/classes.md)
//! and from the 2003 track-making community's notes (HANDOFF.md):
//! - headings within 0..2π; a straight's heading within 0.5° of its direction
//! - X sections: |dlat| <= 1000 m, altitude <= 20 000 m, |grade| <= 60°; F sections |dlat| <= 1000 m
//! - walls at most 100 m tall
//! - nothing on a curve may reach the curve's centre (its dlat on the inside >= the radius)
//! - (warning) curves over 90°: the community split them; Chicago_Street_Circuit loads with 90.4°

use super::model::Track;
use std::collections::HashSet;
use std::f64::consts::TAU;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Level {
    /// The track won't load (or Sandbox refuses it).
    Crash,
    /// Loads, but probably looks or drives wrong.
    Warning,
}

#[derive(Clone, Debug)]
pub struct Problem {
    pub level: Level,
    /// Piece index (0-based), if the problem belongs to one.
    pub piece: Option<usize>,
    pub text: String,
}

fn crash(piece: usize, text: String) -> Problem {
    Problem { level: Level::Crash, piece: Some(piece), text }
}
fn warn(piece: Option<usize>, text: String) -> Problem {
    Problem { level: Level::Warning, piece, text }
}

fn side(d: f64) -> String {
    if d >= 0.0 {
        format!("{d:.1} m to the left")
    } else {
        format!("{:.1} m to the right", -d)
    }
}

pub fn check(t: &Track) -> Vec<Problem> {
    let mut out = Vec::new();
    let n = t.segments.len();
    for s in &t.segments {
        let k = s.index;
        let no = k + 1;
        for (what, h) in [("start", s.start[2]), ("end", s.end[2])] {
            if !(0.0..TAU).contains(&h) || h.is_nan() {
                out.push(crash(k, format!("Piece {no}: the direction at its {what} is out of range")));
            }
        }
        if s.length < 0.01 {
            out.push(crash(k, format!("Piece {no} has no length")));
        }
        if !s.curve {
            let dir = (s.end[1] - s.start[1]).atan2(s.end[0] - s.start[0]);
            let off = |h: f64| ((dir - h + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI).abs();
            let bend = off(s.start[2]).max(off(s.end[2]));
            if s.length >= 0.01 && bend > 0.5f64.to_radians() {
                out.push(crash(k, format!("Piece {no} is a straight but bends by {:.1}°", bend.to_degrees())));
            }
        } else {
            if s.angle.abs() > 90f64.to_radians() + 1e-9 {
                // 2003 track makers split these; Chicago_Street_Circuit loads with 90.4°, so it's
                // advice rather than a hard rule.
                out.push(warn(
                    Some(k),
                    format!("Piece {no} turns {:.1}°. Track makers split curves over 90° into two pieces.", s.angle.abs().to_degrees()),
                ));
            }
            // inside of the turn is +dlat for a left turn
            let inside = s.angle.signum();
            let mut worst: Option<f64> = None;
            for d in s.xs.iter().map(|x| x.dlat).chain(s.fs.iter().map(|f| f.dlat)).chain(s.ws.iter().map(|w| w.dlat)) {
                for v in d {
                    // Exactly at the centre is allowed (COTA has it and loads); past it is not.
                    if inside * v > s.radius + 1e-3 {
                        worst = Some(worst.map_or(v, |w: f64| if v.abs() > w.abs() { v } else { w }));
                    }
                }
            }
            if let Some(v) = worst {
                out.push(crash(
                    k,
                    format!(
                        "Piece {no}: something {} reaches past the centre of this turn (radius {:.1} m). Make the radius bigger or move it in.",
                        side(v),
                        s.radius
                    ),
                ));
            }
        }
        for x in &s.xs {
            for e in 0..2 {
                if x.dlat[e].abs() > 1000.0 || x.alt[e] > 20_000.0 || x.slope[e].abs() > 1.047_197_551 {
                    out.push(crash(k, format!("Piece {no}: a cross-section point at {} is out of range", side(x.dlat[e]))));
                    break;
                }
            }
        }
        // Grade of the road itself (the cross-section points either side of the centerline);
        // run-off and scenery may be as steep as they like.
        {
            let mut by_dlat: Vec<&super::model::XSec> = s.xs.iter().collect();
            by_dlat.sort_by(|a, b| (a.dlat[0] + a.dlat[1]).total_cmp(&(b.dlat[0] + b.dlat[1])));
            let k0 = by_dlat.iter().position(|x| x.dlat[0] + x.dlat[1] > 0.0);
            let road: Vec<&&super::model::XSec> = match k0 {
                Some(0) => by_dlat.iter().take(1).collect(),
                Some(i) => by_dlat[i - 1..=i].iter().collect(),
                None => by_dlat.iter().rev().take(1).collect(),
            };
            let g = road.iter().flat_map(|x| x.slope).fold(0.0f64, |m, v| m.max(v.abs()));
            if g > 0.1 {
                out.push(warn(Some(k), format!("Piece {no}: the road is steep here ({:.0}% grade)", g * 100.0)));
            }
        }
        let xmin = |e: usize| s.xs.iter().map(|x| x.dlat[e]).fold(f64::MAX, f64::min);
        let xmax = |e: usize| s.xs.iter().map(|x| x.dlat[e]).fold(f64::MIN, f64::max);
        if s.xs.is_empty() {
            out.push(warn(Some(k), format!("Piece {no} has no cross-section (no ground height)")));
        } else {
            for f in &s.fs {
                if f.dlat.iter().any(|d| d.abs() > 1000.0) {
                    out.push(crash(k, format!("Piece {no}: a ground strip at {} is out of range", side(f.dlat[0]))));
                }
                for e in 0..2 {
                    if f.dlat[e] < xmin(e) - 0.01 || f.dlat[e] > xmax(e) + 0.01 {
                        out.push(warn(Some(k), format!("Piece {no}: a ground strip at {} is outside the cross-section", side(f.dlat[e]))));
                        break;
                    }
                }
            }
            for w in &s.ws {
                if w.height.iter().any(|h| *h > 100.0) {
                    out.push(crash(k, format!("Piece {no}: a wall at {} is over 100 m tall", side(w.dlat[0]))));
                }
                // Stock tracks often put walls right on the edge, half hanging over; only a wall
                // whose middle is off the ground is worth a warning.
                for e in 0..2 {
                    if w.dlat[e] < xmin(e) - 0.01 || w.dlat[e] > xmax(e) + 0.01 {
                        out.push(warn(Some(k), format!("Piece {no}: a wall at {} is off the edge of the ground", side(w.dlat[e]))));
                        break;
                    }
                }
            }
        }
        // joins
        if k + 1 < n {
            let m = &t.segments[k + 1];
            let gap = (m.start[0] - s.end[0]).hypot(m.start[1] - s.end[1]);
            let turn = ((m.start[2] - s.end[2] + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI).abs();
            if gap > 0.01 {
                out.push(warn(Some(k), format!("Gap of {gap:.2} m between piece {no} and piece {}", no + 1)));
            } else if turn > 0.5f64.to_radians() {
                out.push(warn(Some(k), format!("Kink of {:.1}° between piece {no} and piece {}", turn.to_degrees(), no + 1)));
            }
            let step = (s.height(1.0, 0.0) - m.height(0.0, 0.0)).abs();
            if step > 0.05 {
                out.push(warn(Some(k), format!("Step of {step:.2} m in the road between piece {no} and piece {}", no + 1)));
            }
        }
    }
    if n > 1 {
        let (a, b) = (&t.segments[n - 1], &t.segments[0]);
        let gap = (a.end[0] - b.start[0]).hypot(a.end[1] - b.start[1]);
        if gap > 0.05 {
            out.push(warn(
                Some(n - 1),
                format!("The track doesn't close: the last piece ends {gap:.2} m from the start/finish line"),
            ));
        }
    }
    out.sort_by_key(|p| (p.level, p.piece));
    out
}

/// Textures and objects the track uses that can't be found in its folder, its .dat files or the
/// shared folder. The game says "couldn't load track" when a texture is really missing.
pub fn missing_files(g: &super::graph::Graph, track_dir: &Path, shared_dir: Option<&Path>) -> Vec<Problem> {
    let mut have: HashSet<String> = HashSet::new();
    let add_dir = |dir: &Path, have: &mut HashSet<String>| {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            let name = crate::util::file_name(&p).to_ascii_lowercase();
            if crate::util::ext_lower(&p) == "dat" {
                if let Ok(d) = crate::dat::Dat::open(&p) {
                    for en in &d.entries {
                        have.insert(en.file_name().to_ascii_lowercase());
                    }
                }
            }
            have.insert(name);
        }
    };
    add_dir(track_dir, &mut have);
    add_dir(&track_dir.join("trackmat"), &mut have);
    if let Some(s) = shared_dir {
        add_dir(s, &mut have);
    }
    let mut missing: Vec<String> = Vec::new();
    for n in g.nodes.values() {
        let file = match n.class.as_str() {
            "TextureDescriptor" | "TSODescriptor" => n.s("file"),
            _ => None,
        };
        if let Some(f) = file.filter(|f| !f.is_empty()) {
            let base = f.rsplit(['\\', '/']).next().unwrap_or(f).to_ascii_lowercase();
            if !have.contains(&base) && !missing.contains(&f.to_string()) {
                missing.push(f.to_string());
            }
        }
    }
    missing.sort();
    if missing.is_empty() {
        return Vec::new();
    }
    // One line, not one per file: a .ptf copied out on its own can be missing hundreds.
    let shown: Vec<&str> = missing.iter().take(4).map(String::as_str).collect();
    let more = if missing.len() > shown.len() { format!(" and {} more", missing.len() - shown.len()) } else { String::new() };
    vec![warn(
        None,
        format!(
            "Can't find {} file{} the track uses next to it: {}{more}. If they're really missing the game will say \"couldn't load track\".",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" },
            shown.join(", ")
        ),
    )]
}
