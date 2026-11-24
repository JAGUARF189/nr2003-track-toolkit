//! Apply sample wall / ground / texture edits to a .ptf and save the result (for checking the
//! viewer without clicking): cargo run --release --example walldemo -- <in.ptf> <out.ptf>
use serde_json::json;
use ttk::track::edit::{self, Change};
use ttk::track::objects::apply_nodes;
use ttk::track::surface::{self, Side};
use ttk::track::{check, graph, model::Track, save};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut g = graph::read_ptf_file(std::path::Path::new(&a[1])).unwrap();
    let mut t = Track::from_graph(&g).unwrap();

    // 1. The outside wall of piece 2 becomes a 2 m tall tire wall, copied along the front straight.
    let w = t.segments[1].ws.iter().filter(|w| w.dlat[0] < 0.0).min_by(|x, y| x.dlat[0].total_cmp(&y.dlat[0])).unwrap().id;
    let c = |f: &str, v: serde_json::Value| Change { id: w, field: f.into(), value: v };
    edit::apply(&mut g, &[c("wall_type", json!(2055)), c("height0", json!(2.0)), c("height1", json!(2.0))]).unwrap();
    t = Track::from_graph(&g).unwrap();
    let (ch, missing) = surface::copy_wall_to(&g, &t, w, 0..=3);
    edit::apply(&mut g, &ch).unwrap();
    println!("tire wall copied ({} changes, {missing} pieces without a wall there)", ch.len());

    // 2. A new wall on the left (infield side) of piece 2.
    t = Track::from_graph(&g).unwrap();
    let (edits, id) = surface::add_wall(&g, &t, 1, Side::Left).unwrap();
    apply_nodes(&mut g, &edits, true);
    println!("added wall {id}");

    // 3. The widest strip of piece 2 split, and its left half made grass with the grass texture.
    t = Track::from_graph(&g).unwrap();
    let s = &t.segments[1];
    let (a0, _, _) = s.strips(0.0).into_iter().max_by(|x, y| (x.1 - x.0).total_cmp(&(y.1 - y.0))).unwrap();
    let f = s.fs.iter().find(|f| (f.dlat[0] - a0).abs() < 1e-6).unwrap().id;
    let (edits, half) = surface::split_strip(&g, &t, 1, f).unwrap();
    apply_nodes(&mut g, &edits, true);
    let mut ch = vec![Change { id: half, field: "i15".into(), value: json!(4) }];
    if let Some((tex, _)) = surface::track_textures(&g).into_iter().find(|(_, f)| f.to_ascii_lowercase().contains("grass")) {
        let apps: Vec<i32> = surface::textured_faces(&g, half).into_iter().map(|x| x.1).collect();
        ch.extend(surface::set_texture(&apps, tex));
    }
    edit::apply(&mut g, &ch).unwrap();

    let t = Track::from_graph(&g).unwrap();
    let crashes: Vec<_> = check::check(&t).into_iter().filter(|p| p.level == check::Level::Crash).collect();
    println!("checker: {} won't-load problems", crashes.len());
    let saved = save::save_graph(&g, std::path::Path::new(&a[2])).unwrap();
    println!("saved to {}", saved.path.display());
}
