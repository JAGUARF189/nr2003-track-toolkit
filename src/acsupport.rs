//! What a converted AC track needs besides its .ptf for NR2003 to load and race it:
//!
//! - **Race-control objects:** the start/finish, pit and stop-go flaggers. 131 of 132 installed
//!   tracks place flagger_pit_0 and flagger_stopgo_0; the converter keeps the template's flagger
//!   files and places them by the start line and the pit boxes.
//! - **track.ini:** the template track's, made a road course (track_type 5, chassis_type 4) with
//!   this track's name and length. Pit stalls and grid spots come from AC's AC_PIT_n /
//!   AC_START_n markers as (distance along the lap, distance to the left); any off NR2003's
//!   ground are pulled onto it, and missing ones are filled in after the last.
//! - **.lp files:** NR2003 reads the racing lines by distance along the lap, so they must cover
//!   exactly this lap: one point every 3 m, the last one the rest (checked on Atlanta, Watkins
//!   Glen and Martinsville to 3 cm). A template's .lp files on a longer track stop it loading.
//!   For now they follow the middle of the road at one speed (racing lines from AC's AI line
//!   are phase 5).

use crate::ac::AcScene;
use crate::inifile::IniDoc;
use crate::track::graph::Graph;
use crate::track::model::Track;
use crate::track::objects::{self, NewObject};
use anyhow::{Context, Result};
use std::path::Path;

/// What was made.
#[derive(Debug, Default, Clone)]
pub struct Support {
    /// Pit stalls and grid spots taken from AC's markers (the rest were filled in).
    pub pits_from_ac: usize,
    pub grid_from_ac: usize,
    /// Of those, pulled in onto NR2003's ground.
    pub moved_onto_ground: usize,
    pub flaggers: Vec<String>,
    /// Flagger files the template didn't have.
    pub flaggers_missing: Vec<String>,
    pub lp_files: Vec<String>,
    pub lap_m: f64,
    /// Length of the stretch holding the pit stalls.
    pub pit_lane_m: f64,
    pub ini_written: bool,
}

/// The standard set of racing-line files.
const LP_NAMES: [&str; 10] = ["limp.lp", "maxpanic.lp", "maxpit0.lp", "maxrace.lp", "minpanic.lp", "minpit0.lp", "minrace.lp", "pit0.lp", "race.lp", "raceline.lp"];

/// Where a world point is along the track: (distance along the lap, distance to the left,
/// piece, t along it). None if no piece lies beside it.
fn locate(t: &Track, starts: &[f64], x: f64, y: f64) -> Option<(f64, f64, usize, f64)> {
    let mut best: Option<(f64, f64, usize, f64)> = None;
    for (k, s) in t.segments.iter().enumerate() {
        let (tt, d) = s.locate(x, y);
        if !(-1e-6..=1.0 + 1e-6).contains(&tt) {
            continue;
        }
        if best.is_none_or(|b| d.abs() < b.1.abs()) {
            let tt = tt.clamp(0.0, 1.0);
            best = Some((starts[k] + tt * s.length, d, k, tt));
        }
    }
    best
}

/// (piece, t) at a distance along the lap.
fn at_dlong(t: &Track, starts: &[f64], lap: f64, dlong: f64) -> (usize, f64) {
    let d = dlong.rem_euclid(lap);
    let k = starts.iter().rposition(|&s| s <= d).unwrap_or(0);
    (k, ((d - starts[k]) / t.segments[k].length.max(1e-9)).clamp(0.0, 1.0))
}

/// Keep a spot at least 1.5 m inside NR2003's ground. True if it had to move.
fn onto_ground(t: &Track, k: usize, tt: f64, dlat: f64) -> (f64, bool) {
    let s = &t.segments[k];
    let (lo, hi) = (s.min_dlat(tt) + 1.5, s.max_dlat(tt) - 1.5);
    if lo > hi {
        return (0.0, true);
    }
    let c = dlat.clamp(lo, hi);
    (c, (c - dlat).abs() > 1e-6)
}

/// A file of a track: loose in its folder, else inside its .dat (any file type).
fn track_file(dir: &Path, name: &str) -> Option<Vec<u8>> {
    let loose = dir.join(name);
    if loose.is_file() {
        return std::fs::read(loose).ok();
    }
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if crate::util::ext_lower(&p) != "dat" {
            continue;
        }
        let Ok(dat) = crate::dat::Dat::open(&p) else { continue };
        if let Some(en) = dat.entries.iter().find(|en| en.file_name().eq_ignore_ascii_case(name)) {
            return dat.read(en).ok();
        }
    }
    None
}

/// The number at the end of a marker's name (AC_PIT_12 -> 12).
fn marker_no(name: &str) -> usize {
    name.rsplit('_').next().and_then(|n| n.parse().ok()).unwrap_or(usize::MAX)
}

fn stall_line(n: usize, dlong: f64, dlat: f64) -> String {
    format!("stall_{n} = {dlong:.2} {dlat:.2} 0.00")
}

/// Add flaggers to `graph`, and write `track.ini` and the .lp files into `out` (beside the
/// .ptf; the install step copies them into NR2003). `template_dir` is the track the conversion
/// copied its pieces from (its flagger files, track.ini and .lp names are used). `offset` and
/// `height_base` are the converter's shifts (acconv::Report).
pub fn finish(graph: &mut Graph, scene: &AcScene, offset: [f64; 2], template: &Graph, template_dir: &Path, out: &Path, name: &str) -> Result<Support> {
    let t = Track::from_graph(graph)?;
    let mut starts = Vec::with_capacity(t.segments.len());
    let mut acc = 0.0;
    for s in &t.segments {
        starts.push(acc);
        acc += s.length;
    }
    let lap = acc;
    let mut rep = Support { lap_m: lap, ..Default::default() };

    // --- pit stalls and grid spots from AC's markers
    let spots = |prefix: &str, rep: &mut Support| -> Vec<(f64, f64)> {
        let mut ms: Vec<_> = scene.markers_named(prefix).collect();
        ms.sort_by_key(|m| marker_no(&m.name));
        let mut v = Vec::new();
        for m in ms {
            let (x, y) = (m.pos[0] as f64 - offset[0], m.pos[1] as f64 - offset[1]);
            if let Some((dl, d, k, tt)) = locate(&t, &starts, x, y) {
                let (d, moved) = onto_ground(&t, k, tt, d);
                rep.moved_onto_ground += moved as usize;
                v.push((dl, d));
            }
        }
        v
    };
    let mut pits = spots("AC_PIT_", &mut rep);
    // in driving order along the shortest stretch of the lap that holds them all (AC may number
    // them either way, and they may straddle the start line): start after the biggest gap
    if pits.len() >= 2 {
        pits.sort_by(|a, b| a.0.total_cmp(&b.0));
        let n = pits.len();
        let gap_after = (0..n)
            .max_by(|&i, &j| {
                let g = |k: usize| (pits[(k + 1) % n].0 - pits[k].0).rem_euclid(lap);
                g(i).total_cmp(&g(j))
            })
            .unwrap_or(n - 1);
        pits.rotate_left((gap_after + 1) % n);
    }
    let mut grid = spots("AC_START_", &mut rep);
    rep.pits_from_ac = pits.len();
    rep.grid_from_ac = grid.len();
    // none in AC: on the road after (pits) and behind (grid) the start line
    if pits.is_empty() {
        pits = (0..2).map(|k| (40.0 + 9.0 * k as f64, -2.0)).collect();
    }
    if grid.is_empty() {
        grid = vec![(lap - 15.0, 2.0), (lap - 15.0, -2.0)];
    }
    // fill in more after the last ones, the way they were spaced
    let extend = |v: &mut Vec<(f64, f64)>, want: usize, step: f64, rows_of_two: bool| {
        let base = v.clone();
        while v.len() < want {
            let k = v.len();
            let (dl, d) = if rows_of_two && base.len() >= 2 {
                let prev = v[k - 2];
                (prev.0 + step, prev.1)
            } else {
                let prev = v[k - 1];
                (prev.0 + step, prev.1)
            };
            v.push((dl.rem_euclid(lap), d));
        }
    };
    let pit_step = if pits.len() >= 2 { ((pits[pits.len() - 1].0 - pits[0].0).rem_euclid(lap) / (pits.len() - 1) as f64).clamp(6.0, 20.0) } else { 9.0 };
    rep.pit_lane_m = if pits.len() >= 2 { (pits[pits.len() - 1].0 - pits[0].0).rem_euclid(lap) } else { 0.0 };
    let grid_back = if grid.len() >= 3 { -((grid[0].0 - grid[2].0).rem_euclid(lap)).clamp(5.0, 16.0) } else { -8.0 };

    // --- track.ini: the template's, made this track's
    let src_ini = template_dir.join("track.ini");
    let mut ini = IniDoc::parse(&std::fs::read(&src_ini).with_context(|| format!("reading {}", src_ini.display()))?);
    let miles = lap / 1609.344;
    for (k, v) in [
        ("track_name", name.to_uppercase()),
        ("track_name_short", name.to_string()),
        ("track_city", "Assetto Corsa".into()),
        ("track_state", "conversion".into()),
        ("track_length_n_type", format!("{miles:.2}-mile Road Course")),
        ("track_length", format!("{miles:.3}m")),
        ("track_type", "5".into()),
        ("chassis_type", "4".into()),
        ("track_misc_info_1", String::new()),
        ("track_misc_info_2", String::new()),
        ("track_misc_info_3", String::new()),
        ("starter_decision", format!("{:.1} {:.1}", (lap - 250.0).max(0.0), (lap - 60.0).max(1.0))),
    ] {
        if ini.get("track", k).is_some() || matches!(k, "track_type" | "chassis_type") {
            ini.set("track", k, &v);
        }
    }
    let first_pit = pits[0].0;
    let last_pit = pits[pits.len() - 1].0;
    let pit_side = pits.iter().map(|p| p.1).sum::<f64>() / pits.len() as f64;
    let lane_start = (first_pit - 120.0).rem_euclid(lap);
    let lane_end = (last_pit + 60.0).rem_euclid(lap);
    for sec in ini.section_names() {
        if sec.starts_with("pit_lane") {
            let want = ini.stall_count(&sec).max(pits.len());
            let mut p = pits.clone();
            extend(&mut p, want, pit_step, false);
            let lines: Vec<String> = p.iter().enumerate().map(|(n, s)| stall_line(n, s.0, s.1)).collect();
            ini.replace_stalls(&sec, &lines);
            for (k, v) in [
                ("stall_pace", format!("{:.2} {:.2} 0.00", (first_pit - 15.0).rem_euclid(lap), pits[0].1)),
                ("stall_qual_exit", format!("{:.2} {:.2} 0.00", (last_pit + 15.0).rem_euclid(lap), pits[pits.len() - 1].1)),
                ("pit_lane_start_dlong", format!("{lane_start:.2}")),
                ("pit_lane_end_dlong", format!("{lane_end:.2}")),
                ("merge_to_pit_line_dlong", format!("{:.2}", (lane_start - 150.0).rem_euclid(lap))),
                ("merge_from_pit_line_dlong", format!("{:.2}", (lane_end + 80.0).rem_euclid(lap))),
                ("lane_merge_dlong", format!("{:.2}", (lane_end + 40.0).rem_euclid(lap))),
                ("lane_on_right", if pit_side < 0.0 { "1".into() } else { "0".into() }),
            ] {
                if ini.get(&sec, k).is_some() {
                    ini.set(&sec, k, &v);
                }
            }
        } else if sec.starts_with("starting_grid") {
            let want = ini.stall_count(&sec).max(grid.len());
            let mut g = grid.clone();
            extend(&mut g, want, grid_back, true);
            let lines: Vec<String> = g.iter().enumerate().map(|(n, s)| stall_line(n, s.0, s.1)).collect();
            ini.replace_stalls(&sec, &lines);
            if ini.get(&sec, "stall_pace").is_some() {
                ini.set(&sec, "stall_pace", &format!("{:.2} 0.00 0.00", (grid[0].0 + 20.0).rem_euclid(lap)));
            }
        } else if sec == "apprule enterpits" || sec == "appruleenterpits" {
            for lane in 0..2 {
                for (pt, dl) in [(0, pit_side * 0.5), (1, pit_side * 1.5)] {
                    let kd = format!("pit{lane}_entrance_point{pt}_dlat");
                    let kl = format!("pit{lane}_entrance_point{pt}_dlong");
                    if ini.get(&sec, &kd).is_some() {
                        ini.set(&sec, &kd, &format!("{dl:.2}"));
                        ini.set(&sec, &kl, &format!("{lane_start:.2}"));
                    }
                }
            }
        } else if sec == "spcc" && ini.get(&sec, "num_named_sections").is_some() {
            ini.set(&sec, "num_named_sections", "0");
        } else if sec.starts_with("apprule") && ini.get(&sec, "num_areas").is_some() {
            // the template's own rule areas (Watkins Glen's inner loop) would sit on this road
            ini.set(&sec, "num_areas", "0");
        }
    }
    std::fs::create_dir_all(out)?;
    std::fs::write(out.join("track.ini"), ini.to_bytes())?;
    rep.ini_written = true;

    // --- .lp files covering exactly this lap
    let n = (lap / 3.0).ceil().max(1.0) as usize;
    let rest = lap - 3.0 * (n - 1) as f64;
    let mut lp = Vec::with_capacity(8 + 24 * n);
    lp.extend_from_slice(&3u16.to_le_bytes());
    lp.extend_from_slice(&1u16.to_le_bytes());
    lp.extend_from_slice(&(n as i32).to_le_bytes());
    for k in 0..n {
        for v in [0.6f32, 0.0, 0.0, 0.0, if k + 1 < n { 3.0 } else { rest as f32 }, 0.0] {
            lp.extend_from_slice(&v.to_le_bytes());
        }
    }
    let mut names: Vec<String> = std::fs::read_dir(template_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|f| f.to_ascii_lowercase().ends_with(".lp"))
        .collect();
    if names.is_empty() {
        names = LP_NAMES.iter().map(|s| s.to_string()).collect();
    }
    names.sort_by_key(|s| s.to_ascii_lowercase());
    for f in &names {
        std::fs::write(out.join(f), &lp)?;
    }
    rep.lp_files = names;

    // --- race-control flaggers: the template's files, placed on this track
    let mut have: Vec<String> = template
        .nodes
        .values()
        .filter(|n| n.class == "TSODescriptor")
        .filter_map(|n| n.s("file").map(str::to_string))
        .filter(|f| f.to_ascii_lowercase().starts_with("flagger_"))
        .collect();
    have.sort_by_key(|f| f.to_ascii_lowercase());
    have.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    let wanted = ["flagger_pit_0.3do", "flagger_stopgo_0.3do"];
    let sf = have.iter().find(|f| f.to_ascii_lowercase().starts_with("flagger_sf_")).cloned().unwrap_or_else(|| "flagger_sf_r.3do".into());
    let mut objs = Vec::new();
    for file in wanted.iter().map(|s| s.to_string()).chain([sf.clone()]) {
        let Some(bytes) = track_file(template_dir, &file) else {
            rep.flaggers_missing.push(file);
            continue;
        };
        std::fs::write(out.join(&file), bytes)?;
        let lf = file.to_ascii_lowercase();
        // start/finish: beside the start line, on its side; pit ones by the first and last pit
        let (dlong, side) = if lf.starts_with("flagger_sf_") {
            (2.0, if lf.contains("_l") { 1.0 } else { -1.0 })
        } else if lf.starts_with("flagger_pit_") {
            ((first_pit - 25.0).rem_euclid(lap), pit_side.signum())
        } else {
            ((last_pit + 25.0).rem_euclid(lap), pit_side.signum())
        };
        let (k, tt) = at_dlong(&t, &starts, lap, dlong);
        let s = &t.segments[k];
        let edge = if side >= 0.0 { s.max_dlat(tt) - 1.0 } else { s.min_dlat(tt) + 1.0 };
        let p = s.point(tt, edge);
        let h = s.heading(tt);
        objs.push(NewObject { file: file.clone(), x: p[0], y: p[1], z: s.height(tt, edge), heading: h - side * std::f64::consts::FRAC_PI_2 });
        rep.flaggers.push(file);
    }
    if !objs.is_empty() {
        let (edits, _) = objects::add(graph, &objs)?;
        objects::apply_nodes(graph, &edits, true);
    }
    Ok(rep)
}
