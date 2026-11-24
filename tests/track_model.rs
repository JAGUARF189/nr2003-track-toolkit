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
