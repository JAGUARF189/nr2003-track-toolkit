//! Shape edits and the checker, on real tracks (skipped when the test files aren't here).
use std::path::{Path, PathBuf};
use ttk::track::check::{self, Level};
use ttk::track::edit::{self, Shape};
use ttk::track::{graph, model::Track, save};

fn load(rel: &str) -> Option<(graph::Graph, Track)> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    if !p.is_file() || graph::find_ptfjson().is_none() {
        eprintln!("skipping: {}", p.display());
        return None;
    }
    let g = graph::read_ptf_file(&p).unwrap();
    let t = Track::from_graph(&g).unwrap();
    Some((g, t))
}

fn apply(g: &mut graph::Graph, c: &[edit::Change]) -> Track {
    edit::apply(g, c).unwrap();
    Track::from_graph(g).unwrap()
}

fn max_join_gap(t: &Track) -> f64 {
    t.segments
        .windows(2)
        .map(|w| (w[0].end[0] - w[1].start[0]).hypot(w[0].end[1] - w[1].start[1]))
        .fold(0.0, f64::max)
}

fn crashes(t: &Track) -> Vec<String> {
    check::check(t).into_iter().filter(|p| p.level == Level::Crash).map(|p| p.text).collect()
}

#[test]
fn stock_tracks_have_no_crash_problems() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("_test/ptfs");
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        for f in std::fs::read_dir(e.path()).into_iter().flatten().flatten() {
            let p = f.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ptf")) && graph::find_ptfjson().is_some() {
                let g = graph::read_ptf_file(&p).unwrap();
                let t = Track::from_graph(&g).unwrap();
                let all = check::check(&t);
                let c: Vec<_> = all.iter().filter(|p| p.level == Level::Crash).collect();
                eprintln!("{}: {} warnings", p.display(), all.len() - c.len());
                assert!(c.is_empty(), "{}: {c:#?}", p.display());
            }
        }
    }
}

#[test]
fn longer_straight_moves_the_rest_of_the_track() {
    let Some((mut g, t)) = load("_test/ptfs/martinsville/martinsville.ptf") else { return };
    let k = 14; // piece 15, the 146.3 m back straight
    let before = t.segments[k].length;
    let changes = edit::reshape(&t, k, Shape::Straight { length: before + 10.0 }, true).unwrap();
    let t2 = apply(&mut g, &changes);
    assert!((t2.segments[k].length - (before + 10.0)).abs() < 1e-6);
    assert!((t2.length - (t.length + 10.0)).abs() < 1e-6);
    assert!(max_join_gap(&t2) < 1e-6, "joins opened: {}", max_join_gap(&t2));
    // Every later piece keeps its own shape
    for j in k + 1..t.segments.len() {
        assert!((t2.segments[j].length - t.segments[j].length).abs() < 1e-6);
        assert!((t2.segments[j].angle - t.segments[j].angle).abs() < 1e-9);
    }
    // A straight edit doesn't rotate anything, so the loop now has a 10 m gap at the line
    let p = check::check(&t2);
    assert!(p.iter().any(|p| p.text.contains("doesn't close")), "{p:#?}");
    assert!(crashes(&t2).is_empty(), "{:#?}", crashes(&t2));
}

#[test]
fn tighter_turn_and_too_small_radius_are_caught() {
    let Some((mut g, t)) = load("_test/ptfs/martinsville/martinsville.ptf") else { return };
    let k = 7; // piece 8: left 45° radius 57
    let s = &t.segments[k];
    // 40° instead of 45°: headings stay in range, joins stay exact
    let c = edit::reshape(&t, k, Shape::Curve { angle: 40f64.to_radians(), radius: s.radius }, true).unwrap();
    let t2 = apply(&mut g, &c);
    assert!((t2.segments[k].angle.to_degrees() - 40.0).abs() < 1e-6);
    assert!(max_join_gap(&t2) < 1e-6);
    assert!(t2.segments.iter().all(|s| (0.0..std::f64::consts::TAU).contains(&s.start[2])));
    assert!(crashes(&t2).is_empty(), "{:#?}", crashes(&t2));
    // Radius 10 m: the infield sections (up to ~47 m to the left) reach past the centre
    let c = edit::reshape(&t2, k, Shape::Curve { angle: 40f64.to_radians(), radius: 10.0 }, true).unwrap();
    let t3 = apply(&mut g, &c);
    assert!(crashes(&t3).iter().any(|m| m.contains("reaches past the centre")), "{:#?}", crashes(&t3));
    // 120° in one piece must be split
    let c = edit::reshape(&t3, k, Shape::Curve { angle: 120f64.to_radians(), radius: 80.0 }, true).unwrap();
    let t4 = apply(&mut g, &c);
    assert!(check::check(&t4).iter().any(|p| p.text.contains("over 90")));
}

#[test]
fn banking_edit_is_exact_and_leaves_no_step() {
    let Some((mut g, t)) = load("_test/ptfs/martinsville/martinsville.ptf") else { return };
    let k = 7;
    let inside_before = t.segments[k].height(0.5, 13.7);
    let c = edit::set_banking(&t, k, 20.0, 18.0).unwrap();
    let t2 = apply(&mut g, &c);
    let s = &t2.segments[k];
    assert!((edit::banking_outside(s, 0.0) - 20.0).abs() < 0.01, "{}", edit::banking_outside(s, 0.0));
    assert!((edit::banking_outside(s, 1.0) - 18.0).abs() < 0.01, "{}", edit::banking_outside(s, 1.0));
    // the inside edge of the racing surface didn't move
    assert!((s.height(0.5, 13.7) - inside_before).abs() < 0.05);
    // no step at either join, at several points across the road
    for d in [-3.0, 0.0, 5.0, 12.0] {
        assert!((t2.segments[k - 1].height(1.0, d) - s.height(0.0, d)).abs() < 1e-6);
        assert!((s.height(1.0, d) - t2.segments[k + 1].height(0.0, d)).abs() < 1e-6);
    }
    assert!(!check::check(&t2).iter().any(|p| p.text.contains("Step")));
}

#[test]
fn edited_track_saves_and_reads_back() {
    let Some((mut g, t)) = load("_test/ptfs/martinsville/martinsville.ptf") else { return };
    let mut c = edit::reshape(&t, 14, Shape::Straight { length: 150.0 }, true).unwrap();
    c.extend(edit::set_banking(&t, 7, 15.0, 15.0).unwrap());
    let t2 = apply(&mut g, &c);
    let dir: PathBuf = std::env::temp_dir().join(format!("ttk_edit_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("martinsville.ptf");
    save::save_graph(&g, &dest).unwrap();
    let back = Track::from_graph(&graph::read_ptf_file(&dest).unwrap()).unwrap();
    assert!((back.length - t2.length).abs() < 1e-9);
    assert!((edit::banking_outside(&back.segments[7], 0.5) - 15.0).abs() < 0.05, "{}", edit::banking_outside(&back.segments[7], 0.5));
    let _ = std::fs::remove_dir_all(&dir);
}
