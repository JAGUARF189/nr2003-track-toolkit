//! End-to-end: .ptf -> ptfjson -> graph -> model -> mesh, on test tracks in `_test\`
//! (skipped when they aren't on this machine).
use std::path::Path;
use ttk::track::{graph, mesh, model};

fn load(rel: &str) -> Option<model::Track> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    if !p.is_file() || graph::find_ptfjson().is_none() {
        eprintln!("skipping: {} or ptfjson.exe not found", p.display());
        return None;
    }
    let (bytes, _) = graph::load_ptf_bytes(&p).unwrap();
    let g = graph::parse_ptf(&bytes).unwrap();
    Some(model::Track::from_graph(&g).unwrap())
}

#[test]
fn martinsville_is_half_a_mile_and_banked() {
    let Some(t) = load("_test/ptfs/martinsville/martinsville.ptf") else { return };
    assert_eq!(t.segments.len(), 32);
    let miles = t.length / model::METRES_PER_MILE;
    assert!((miles - 0.526).abs() < 0.001, "length {miles} mi");
    let turn: f64 = t.segments.iter().map(|s| s.angle).sum::<f64>().to_degrees();
    assert!((turn - 360.0).abs() < 0.01, "total turn {turn}");
    // Segments are joined end to start
    for w in t.segments.windows(2) {
        let a = w[0].point(1.0, 0.0);
        let b = w[1].point(0.0, 0.0);
        assert!((a[0] - b[0]).hypot(a[1] - b[1]) < 1e-3);
    }
    // Martinsville's turns are banked about 12 degrees, higher on the outside (right)
    let max_bank = t.segments.iter().map(|s| s.banking(0.5)).fold(f64::MIN, f64::max);
    assert!(max_bank > 10.0 && max_bank < 14.0, "max banking {max_bank}");
    let m = mesh::build(&t, true);
    let o = mesh::objects(&t, m.origin, |_| mesh::OBJECT_COLOR);
    assert!(o.len() >= t.objects.len() * 30 * mesh::VERTEX_FLOATS);
    assert!(m.verts.len() > 10_000 * mesh::VERTEX_FLOATS);
    assert!(m.verts.iter().all(|v| v.is_finite()));
}

#[test]
fn hillside_is_one_mile_from_the_dat() {
    let Some(t) = load("_test/extract_hillside/hillside.ptf") else { return };
    assert!((t.length / model::METRES_PER_MILE - 1.0).abs() < 0.001);
    assert_eq!(t.objects.len(), 263);
    assert!(t.objects.iter().any(|o| o.file.ends_with(".3do")));
}

#[test]
fn martinsville_textures_are_found_and_mapped() {
    let Some(t) = load("_test/ptfs/martinsville/martinsville.ptf") else { return };
    let strips = t.segments.iter().flat_map(|s| s.fs.iter()).filter(|f| f.tex.is_some()).count();
    assert!(strips > 400, "{strips} textured strips");
    // The stored corners match the layer fields: the first asphalt strip of piece 1 tiles
    // every 8.19 m across (texel 0.008 x 1024) starting 2.2 m to the right of its edge.
    let f = t.segments[0].fs.iter().find(|f| f.tex.as_ref().is_some_and(|x| x.file == r"trackmat\asphalt.mip")).unwrap();
    let tx = f.tex.as_ref().unwrap();
    assert!((tx.uv[0][0] - (f.dlat[0] + 2.2) / 8.192).abs() < 1e-3, "{:?}", tx.uv);
    // walls carry their face textures
    let faces = t.segments.iter().flat_map(|s| s.ws.iter()).flat_map(|w| w.parts.iter()).flat_map(|p| p.faces.iter()).filter(|f| f.is_some()).count();
    assert!(faces > 100, "{faces} textured wall faces");
    let m = mesh::build(&t, true);
    let files = m.textures();
    let store = ttk::track::textures::TexStore::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("_test/ptfs/martinsville").as_path());
    let loaded = files.iter().filter(|f| store.load(f, 256).is_some()).count();
    assert_eq!(loaded, files.len(), "{files:?}");
    assert_eq!(m.batches.iter().map(|b| b.count as usize).sum::<usize>(), m.verts.len() / mesh::VERTEX_FLOATS);
}
