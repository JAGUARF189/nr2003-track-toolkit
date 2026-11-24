//! Walls, ground strips and textures, on a real track (skipped when the test files aren't here).
use std::path::{Path, PathBuf};
use ttk::track::objects::{apply_nodes, NodeEdit};
use ttk::track::surface::{self, Side};
use ttk::track::{edit, graph, model::Track, save};

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
    let d = std::env::temp_dir().join(format!("ttk_surface_test_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Save to a temporary file and read it back.
fn round_trip(g: &graph::Graph, name: &str) -> (graph::Graph, Track) {
    let dir = tmp(name);
    let dest = dir.join("martinsville.ptf");
    save::save_graph(g, &dest).unwrap();
    let back = graph::read_ptf_file(&dest).unwrap();
    assert!(g.canonical().difference(&back).is_none());
    let t = Track::from_graph(&back).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (back, t)
}

fn in_dlat_order(t: &Track) -> bool {
    t.segments.iter().all(|s| {
        let f: Vec<f64> = s.fs.iter().map(|f| f.dlat[0] + f.dlat[1]).collect();
        let w: Vec<f64> = s.ws.iter().map(|w| w.dlat[0] + w.dlat[1]).collect();
        f.windows(2).all(|p| p[0] <= p[1]) && w.windows(2).all(|p| p[0] <= p[1])
    })
}

fn apply(g: &mut graph::Graph, edits: &[NodeEdit]) -> Track {
    apply_nodes(g, edits, true);
    Track::from_graph(g).unwrap()
}

const MV: &str = "_test/ptfs/martinsville/martinsville.ptf";

#[test]
fn add_and_delete_walls() {
    let Some((g0, t0)) = load(MV) else { return };
    let mut g = g0.clone();
    let k = 5;
    let before = t0.segments[k].ws.len();
    let (edits, id) = surface::add_wall(&g, &t0, k, Side::Left).unwrap();
    let t = apply(&mut g, &edits);
    assert_eq!(t.segments[k].ws.len(), before + 1);
    let w = t.segments[k].ws.iter().find(|w| w.id == id).unwrap();
    // on the left edge of the ground
    let s = &t.segments[k];
    assert!((w.dlat[0] + 0.5 * w.width[0] - s.max_dlat(0.0)).abs() < 1e-3, "{:?}", w.dlat);
    assert!(in_dlat_order(&t));
    // its faces are its own (new appearances), but textures are shared
    let faces = surface::textured_faces(&g, id);
    assert!(!faces.is_empty());
    for (_, app) in &faces {
        assert!(g0.node(*app).is_none(), "appearance should be new");
        let (tex, _) = surface::texture_of(&g, *app).unwrap();
        assert!(g0.node(tex).is_some(), "texture should be shared");
    }
    let (_, tb) = round_trip(&g, "addwall");
    assert_eq!(tb.segments[k].ws.len(), before + 1);

    // delete it again: everything it owned goes too
    let del = surface::delete_part(&g, &t, k, id).unwrap();
    let t2 = apply(&mut g, &del);
    assert_eq!(t2.segments[k].ws.len(), before);
    assert_eq!(g.nodes.len(), g0.nodes.len());

    // Undo of the add brings back exactly the original
    apply_nodes(&mut g, &del, false);
    apply_nodes(&mut g, &edits, false);
    assert!(g0.difference(&g).is_none());

    // deleting a stock wall saves
    let mut g3 = g0.clone();
    let wid = t0.segments[k].ws[0].id;
    let t3 = apply(&mut g3, &surface::delete_part(&g0, &t0, k, wid).unwrap());
    assert_eq!(t3.segments[k].ws.len(), before - 1);
    round_trip(&g3, "delwall");
}

#[test]
fn split_and_remove_strips() {
    let Some((g0, t0)) = load(MV) else { return };
    let mut g = g0.clone();
    let k = 2;
    let s = &t0.segments[k];
    // the widest strip
    let strips = s.strips(0.0);
    let (a, _, _) = strips.iter().copied().max_by(|x, y| (x.1 - x.0).total_cmp(&(y.1 - y.0))).unwrap();
    let f = s.fs.iter().find(|f| (f.dlat[0] - a).abs() < 1e-6).unwrap();
    let (edits, new_id) = surface::split_strip(&g, &t0, k, f.id).unwrap();
    let t = apply(&mut g, &edits);
    assert_eq!(t.segments[k].fs.len(), s.fs.len() + 1);
    let nf = t.segments[k].fs.iter().find(|x| x.id == new_id).unwrap();
    assert_eq!(nf.surface, f.surface);
    assert!(in_dlat_order(&t));
    round_trip(&g, "split");

    let del = surface::delete_part(&g, &t, k, new_id).unwrap();
    apply_nodes(&mut g, &del, true);
    apply_nodes(&mut g, &del, false);
    apply_nodes(&mut g, &edits, false);
    assert!(g0.difference(&g).is_none());

    // changing the surface and moving an edge are plain number changes
    let mut g2 = g0.clone();
    let (lo, hi) = surface::strip_edge_limits(&t0, k, f.id, 0);
    assert!(lo < f.dlat[0] && f.dlat[0] < hi);
    edit::apply(&mut g2, &[
        edit::Change { id: f.id, field: "i15".into(), value: serde_json::json!(4) },
        edit::Change { id: f.id, field: "dlat0".into(), value: serde_json::json!(0.5 * (f.dlat[0] + hi)) },
    ])
    .unwrap();
    let (_, tb) = round_trip(&g2, "surface");
    assert_eq!(tb.segments[k].fs.iter().find(|x| (x.dlat[1] - f.dlat[1]).abs() < 1e-6).unwrap().surface, 4);
}

#[test]
fn change_textures() {
    let Some((g0, t0)) = load(MV) else { return };
    let texs = surface::track_textures(&g0);
    assert!(texs.len() > 3, "{texs:?}");
    let k = 3;
    let f = &t0.segments[k].fs[0];
    let (_, app) = surface::textured_faces(&g0, f.id)[0].clone();
    let (old_tex, _) = surface::texture_of(&g0, app).unwrap();
    let other = texs.iter().find(|t| t.0 != old_tex).unwrap().clone();

    let mut g = g0.clone();
    edit::apply(&mut g, &surface::set_texture(&[app], other.0)).unwrap();
    assert_eq!(surface::texture_of(&g, app).unwrap().1, other.1);
    // only this strip changed
    let users = |g: &graph::Graph, tex: i32| {
        g.nodes.values().filter(|n| n.class == "AppearanceDescriptor" && n.r("tex[0]") == Some(tex)).count()
    };
    assert_eq!(users(&g, other.0), users(&g0, other.0) + 1);
    round_trip(&g, "tex");

    // a texture file the track doesn't have yet
    let (node, tex) = surface::texture_for_file(&g, "trackmat\\my_new_grass.mip");
    let node = node.expect("new texture");
    apply_nodes(&mut g, &[node], true);
    edit::apply(&mut g, &surface::set_texture(&[app], tex)).unwrap();
    let (back, _) = round_trip(&g, "newtex");
    assert!(back.nodes.values().any(|n| n.s("file") == Some("trackmat\\my_new_grass.mip")));
    // asking again finds it (any case)
    let (again, id) = surface::texture_for_file(&g, "TRACKMAT\\my_new_grass.mip");
    assert!(again.is_none() && id == tex);
}

#[test]
fn copy_a_wall_and_a_strip_along_the_track() {
    let Some((g0, t0)) = load(MV) else { return };
    let mut g = g0.clone();
    // the outside wall of piece 1, made a tire wall 2 m tall, copied to pieces 2..=10
    let s = &t0.segments[0];
    let w = s.ws.iter().min_by(|a, b| a.dlat[0].total_cmp(&b.dlat[0])).unwrap();
    edit::apply(&mut g, &[
        edit::Change { id: w.id, field: "wall_type".into(), value: serde_json::json!(2055) },
        edit::Change { id: w.id, field: "height0".into(), value: serde_json::json!(2.0) },
        edit::Change { id: w.id, field: "height1".into(), value: serde_json::json!(2.0) },
    ])
    .unwrap();
    let t = Track::from_graph(&g).unwrap();
    let (changes, missing) = surface::copy_wall_to(&g, &t, w.id, 1..=9);
    edit::apply(&mut g, &changes).unwrap();
    let t = Track::from_graph(&g).unwrap();
    let mut done = 0;
    for s in &t.segments[1..=9] {
        if let Some(r) = s.ws.iter().filter(|x| x.dlat[0] + x.dlat[1] < 0.0).min_by(|a, b| a.dlat[0].total_cmp(&b.dlat[0])) {
            if r.wall_type == 2055 && (r.height[0] - 2.0).abs() < 1e-9 {
                done += 1;
            }
        }
    }
    assert_eq!(done + missing, 9, "done {done}, missing {missing}");
    round_trip(&g, "copywall");

    // the racing surface of piece 1 made concrete, copied along
    let f = s.fs.iter().find(|f| f.surface == 1).unwrap();
    edit::apply(&mut g, &[edit::Change { id: f.id, field: "i15".into(), value: serde_json::json!(2) }]).unwrap();
    let t = Track::from_graph(&g).unwrap();
    let c = surface::copy_strip_to(&g, &t, f.id, 1..=4);
    assert!(!c.is_empty());
    edit::apply(&mut g, &c).unwrap();
    round_trip(&g, "copystrip");
}

#[test]
fn bringing_in_a_texture_copies_it_to_trackmat() {
    let src = tmp("tex_src");
    let track = tmp("tex_track");
    std::fs::write(src.join("wall_new.mip"), b"mip").unwrap();
    let (name, note) = surface::bring_in_texture(&src.join("wall_new.mip"), &track).unwrap();
    assert_eq!(name, "trackmat\\wall_new.mip");
    assert!(note.is_some() && track.join("trackmat").join("wall_new.mip").is_file());
    // from the track's own trackmat: used where it is
    let (name2, note2) = surface::bring_in_texture(&track.join("trackmat").join("wall_new.mip"), &track).unwrap();
    assert_eq!(name2, "trackmat\\wall_new.mip");
    assert!(note2.is_none());
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&track);
}

#[test]
fn a_wall_moved_past_another_keeps_the_order() {
    let Some((g0, t0)) = load(MV) else { return };
    let k = 1;
    let s = &t0.segments[k];
    assert!(s.ws.len() >= 2);
    let w = &s.ws[0];
    let past = s.ws.iter().map(|x| 0.5 * (x.dlat[0] + x.dlat[1])).fold(f64::MIN, f64::max) + 1.0;
    let mut g = g0.clone();
    let resort = surface::resort_walls(&g, &t0, k, w.id, past).expect("order changes");
    assert!(surface::resort_walls(&g, &t0, k, w.id, 0.5 * (w.dlat[0] + w.dlat[1])).is_none());
    apply_nodes(&mut g, &[resort], true);
    edit::apply(&mut g, &[
        edit::Change { id: w.id, field: "dlat0".into(), value: serde_json::json!(past) },
        edit::Change { id: w.id, field: "dlat1".into(), value: serde_json::json!(past) },
    ])
    .unwrap();
    let t = Track::from_graph(&g).unwrap();
    assert!(in_dlat_order(&t));
    assert_eq!(t.segments[k].ws.last().unwrap().id, w.id);
    round_trip(&g, "resort");
}
