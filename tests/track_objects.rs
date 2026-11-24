//! Placing, copying and removing objects, on real tracks (skipped when the test files aren't here).
use std::path::{Path, PathBuf};
use ttk::track::edit;
use ttk::track::objects::{self, NewObject};
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

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ttk_objects_test_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn tso_count(g: &graph::Graph) -> usize {
    g.nodes.values().filter(|n| n.class == "TSODescriptor").count()
}

const MV: &str = "_test/ptfs/martinsville/martinsville.ptf";

#[test]
fn a_file_read_from_disk_is_already_in_writer_order() {
    let Some((g, _)) = load(MV) else { return };
    assert!(g.difference(&g.canonical()).is_none());
}

#[test]
fn added_objects_save_and_load_back() {
    let Some((g0, t0)) = load(MV) else { return };
    let mut g = g0.clone();
    let existing = t0.objects[0].file.clone();
    let objs = vec![
        NewObject { file: existing.to_uppercase(), x: 10.0, y: 20.0, z: 1.5, heading: 1.0 },
        NewObject { file: "my_new_thing.3do".into(), x: -30.0, y: 5.0, z: 0.0, heading: -0.5 },
        NewObject { file: "my_new_thing.3do".into(), x: -40.0, y: 5.0, z: 0.0, heading: 0.0 },
    ];
    let (edits, ids) = objects::add(&g, &objs).unwrap();
    assert_eq!(ids.len(), 3);
    objects::apply_nodes(&mut g, &edits, true);
    let t = Track::from_graph(&g).unwrap();
    assert_eq!(t.objects.len(), t0.objects.len() + 3);
    // an existing file reuses its TSO (any case); the new file gets one, shared by both copies
    assert_eq!(tso_count(&g), tso_count(&g0) + 1);

    let dir = tmp("add");
    let dest = dir.join("martinsville.ptf");
    save::save_graph(&g, &dest).unwrap();
    let back = graph::read_ptf_file(&dest).unwrap();
    assert!(g.canonical().difference(&back).is_none());
    let tb = Track::from_graph(&back).unwrap();
    assert_eq!(tb.objects.len(), t0.objects.len() + 3);
    let last = tb.objects.last().unwrap();
    assert_eq!(last.file, "my_new_thing.3do");
    assert!((last.x + 40.0).abs() < 1e-9 && (last.y - 5.0).abs() < 1e-9);
    // the track itself didn't move
    assert!((tb.length - t0.length).abs() < 1e-9);

    // Undo brings back exactly the original
    objects::apply_nodes(&mut g, &edits, false);
    assert!(g0.difference(&g).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn deleting_objects_drops_unused_files_and_saves() {
    let Some((g0, t0)) = load(MV) else { return };
    let mut g = g0.clone();
    // every placement of the first object's file, plus one other placement
    let file = t0.objects[0].file.clone();
    let mut which: Vec<i32> = t0.objects.iter().filter(|o| o.file == file).map(|o| o.id).collect();
    let other = t0.objects.iter().find(|o| o.file != file).unwrap();
    which.push(other.id);
    let shares_other = t0.objects.iter().filter(|o| o.file == other.file).count() > 1;
    let edits = objects::delete(&g, &which).unwrap();
    objects::apply_nodes(&mut g, &edits, true);
    let t = Track::from_graph(&g).unwrap();
    assert_eq!(t.objects.len(), t0.objects.len() - which.len());
    assert!(!t.objects.iter().any(|o| o.file == file));
    let dropped = 1 + usize::from(!shares_other);
    assert_eq!(tso_count(&g), tso_count(&g0) - dropped);

    let dir = tmp("delete");
    let dest = dir.join("martinsville.ptf");
    save::save_graph(&g, &dest).unwrap();
    let tb = Track::from_graph(&graph::read_ptf_file(&dest).unwrap()).unwrap();
    assert_eq!(tb.objects.len(), t.objects.len());

    objects::apply_nodes(&mut g, &edits, false);
    assert!(g0.difference(&g).is_none());

    // removing every object leaves an empty list that still saves
    let all: Vec<i32> = t0.objects.iter().map(|o| o.id).collect();
    let mut g2 = g0.clone();
    let e2 = objects::delete(&g2, &all).unwrap();
    objects::apply_nodes(&mut g2, &e2, true);
    save::save_graph(&g2, &dest).unwrap();
    let tb = Track::from_graph(&graph::read_ptf_file(&dest).unwrap()).unwrap();
    assert!(tb.objects.is_empty());
    assert_eq!(tso_count(&g2), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn duplicate_move_and_turn() {
    let Some((g0, t0)) = load(MV) else { return };
    let mut g = g0.clone();
    let which = vec![t0.objects[3].id, t0.objects[4].id];
    let (edits, ids) = objects::duplicate(&g, &which, 5.0, -5.0).unwrap();
    objects::apply_nodes(&mut g, &edits, true);
    let t = Track::from_graph(&g).unwrap();
    let copy = t.objects.iter().find(|o| o.id == ids[0]).unwrap().clone();
    assert!((copy.x - (t0.objects[3].x + 5.0)).abs() < 1e-9);
    assert_eq!(copy.file, t0.objects[3].file);
    assert_eq!(tso_count(&g), tso_count(&g0));

    let mv = objects::move_by(&g, &ids, 1.0, 2.0, 0.5);
    edit::apply(&mut g, &mv).unwrap();
    let (cx, cy) = (copy.x + 1.0, copy.y + 2.0);
    let turn = objects::turn_by(&g, &ids[..1], cx, cy, std::f64::consts::FRAC_PI_2);
    edit::apply(&mut g, &turn).unwrap();
    let t = Track::from_graph(&g).unwrap();
    let c2 = t.objects.iter().find(|o| o.id == ids[0]).unwrap();
    // turning about its own position only changes the direction
    assert!((c2.x - cx).abs() < 1e-9 && (c2.y - cy).abs() < 1e-9);
    let want = edit::wrap_heading(t0.objects[3].heading + std::f64::consts::FRAC_PI_2);
    assert!((c2.heading - want).abs() < 1e-9);

    let dir = tmp("dup");
    save::save_graph(&g, &dir.join("martinsville.ptf")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ground_under_the_track_matches_the_track() {
    let Some((_, t)) = load(MV) else { return };
    for s in t.segments.iter().step_by(3) {
        for (tt, d) in [(0.3, 0.0), (0.7, 4.0), (0.5, -6.0)] {
            let p = s.point(tt, d);
            let (lt, ld) = s.locate(p[0], p[1]);
            assert!((lt - tt).abs() < 1e-6 && (ld - d).abs() < 1e-6, "piece {} t {lt} d {ld}", s.index);
            let h = t.ground_at(p[0], p[1]);
            assert!((h - s.height(tt, d)).abs() < 1e-6, "piece {}: {h} vs {}", s.index, s.height(tt, d));
        }
    }
    // the stock objects mostly sit near the ground already
    let near = t.objects.iter().filter(|o| (t.ground_at(o.x, o.y) - o.z).abs() < 1.0).count();
    eprintln!("{near} of {} objects within 1 m of the ground", t.objects.len());
    assert!(near * 2 > t.objects.len(), "{near} of {}", t.objects.len());
}

#[test]
fn a_stock_object_names_its_texture() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("_test/ptfs/martinsville/building07a.3do");
    let Ok(b) = std::fs::read(&p) else { return };
    let used = objects::files_used_by(&b);
    assert!(used.iter().any(|f| f.eq_ignore_ascii_case("building07a_01.mip")), "{used:?}");
}

#[test]
fn bringing_in_an_object_copies_its_textures() {
    let src = tmp("bring_src");
    let track = tmp("bring_track");
    let mut model = vec![1u8, 0, 0, 0, 2, 0, 0, 0];
    model.extend_from_slice(b"\0wall_tex.mip\0\x07other.MIP\0");
    std::fs::write(src.join("thing.3do"), &model).unwrap();
    std::fs::write(src.join("wall_tex.mip"), b"tex").unwrap();
    std::fs::write(track.join("other.mip"), b"different").unwrap();
    std::fs::write(src.join("other.mip"), b"mine").unwrap();
    assert_eq!(objects::files_used_by(&model), vec!["wall_tex.mip".to_string(), "other.MIP".to_string()]);
    let r = objects::bring_in(&src.join("thing.3do"), &track, None).unwrap();
    assert_eq!(r.file, "thing.3do");
    assert!(track.join("thing.3do").is_file() && track.join("wall_tex.mip").is_file());
    assert_eq!(r.kept_existing, vec!["other.MIP".to_string()]);
    assert_eq!(std::fs::read(track.join("other.mip")).unwrap(), b"different");
    // from the track's own folder: nothing to copy
    let r2 = objects::bring_in(&track.join("thing.3do"), &track, None).unwrap();
    assert!(r2.copied.is_empty());
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&track);
}

#[test]
fn locks_are_kept_next_to_the_track() {
    let Some((_, t)) = load(MV) else { return };
    let dir = tmp("locks");
    let dest = dir.join("martinsville.ptf");
    let locked: std::collections::HashSet<i32> = [t.objects[0].id, t.objects[7].id].into_iter().collect();
    objects::save_locks(&dest, &t, &locked).unwrap();
    assert!(objects::locks_path(&dest).is_file());
    assert_eq!(objects::load_locks(&dest, &t), locked);
    objects::save_locks(&dest, &t, &Default::default()).unwrap();
    assert!(!objects::locks_path(&dest).is_file());
    let _ = std::fs::remove_dir_all(&dir);
}
