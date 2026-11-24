//! AC -> NR2003 conversion on the local AC install (skipped when it isn't there).
use std::path::{Path, PathBuf};
use std::time::Instant;
use ttk::{ac, acconv, track};

fn scene(name: &str, layout: &str) -> Option<ac::AcScene> {
    let dir = ac::find_tracks_dir()?.join(name);
    let tpl = Path::new(env!("CARGO_MANIFEST_DIR")).join("_test/extract_hillside/hillside.ptf");
    if !dir.is_dir() || ac::find_acdump().is_none() || !tpl.is_file() {
        eprintln!("skipping: {name}, acdump.exe or the hillside template missing");
        return None;
    }
    let out = std::env::temp_dir().join(format!("ttk_acconv_{}_{name}.acscene", std::process::id()));
    ac::export_scene(&dir, layout, &out, 0).unwrap();
    let s = ac::AcScene::read(&out, 0).unwrap();
    let _ = std::fs::remove_file(&out);
    Some(s)
}

fn template() -> track::graph::Graph {
    track::graph::read_ptf_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("_test/extract_hillside/hillside.ptf")).unwrap()
}

fn convert_and_check(name: &str, layout: &str) {
    let Some(s) = scene(name, layout) else { return };
    let t0 = Instant::now();
    let c = acconv::convert(&s, &template(), &acconv::Options::default()).unwrap();
    let r = &c.report;
    eprintln!(
        "{name}: {} pieces ({} straight, {} curved), worst {:.2} m, mean {:.3} m, length AC {:.1} / NR {:.1}, heights found {:.0}%, worst height {:.2} m, {} warnings, {:.1}s",
        r.pieces, r.straights, r.curves, r.worst_m, r.mean_m, r.length_ac, r.length_nr, r.heights_found * 100.0, r.worst_height_m, r.warnings, t0.elapsed().as_secs_f64()
    );
    assert!(r.crashes.is_empty(), "won't load: {:#?}", r.crashes);
    assert!(r.worst_m < 0.5, "worst {}", r.worst_m);
    assert!((r.length_nr - r.length_ac).abs() / r.length_ac < 0.005);
    // closes: last piece ends where the first starts, no kinks
    let t = track::model::Track::from_graph(&c.graph).unwrap();
    let (a, b) = (t.segments.last().unwrap(), &t.segments[0]);
    assert!((a.end[0] - b.start[0]).hypot(a.end[1] - b.start[1]) < 0.02);
    let problems = track::check::check(&t);
    for p in problems.iter().take(10) {
        eprintln!("  {:?}: {}", p.level, p.text);
    }
    // saves and reads back
    let dir: PathBuf = std::env::temp_dir().join(format!("ttk_acconv_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join(format!("{name}.ptf"));
    track::save::save_graph(&c.graph, &dest).unwrap();
    let back = track::model::Track::from_graph(&track::graph::read_ptf_file(&dest).unwrap()).unwrap();
    assert_eq!(back.segments.len(), t.segments.len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mugello_converts() {
    convert_and_check("mugello", "");
}

#[test]
fn monaco_2026_converts() {
    convert_and_check("monaco_2020", "monaco_f1_2026");
}
