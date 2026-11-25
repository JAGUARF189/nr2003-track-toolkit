//! AC scenery -> NR2003 objects, on the local AC install (skipped when it isn't there).
//! No Sandbox tools are run here: this checks what make3do would be given.
use std::path::Path;
use ttk::{ac, acconv, acscenery, track};

fn scene(name: &str) -> Option<ac::AcScene> {
    let dir = ac::find_tracks_dir()?.join(name);
    let tpl = Path::new(env!("CARGO_MANIFEST_DIR")).join("_test/extract_hillside/hillside.ptf");
    if !dir.is_dir() || ac::find_acdump().is_none() || !tpl.is_file() {
        eprintln!("skipping: {name}, acdump.exe or the hillside template missing");
        return None;
    }
    let out = std::env::temp_dir().join(format!("ttk_acscenery_{}_{name}.acscene", std::process::id()));
    ac::export_scene(&dir, "", &out, 0).unwrap();
    let s = ac::AcScene::read(&out, 0).unwrap();
    let _ = std::fs::remove_file(&out);
    Some(s)
}

/// Point-in-triangle height (None outside).
fn height_in(p: [f64; 2], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> Option<f64> {
    let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if d.abs() < 1e-12 {
        return None;
    }
    let l1 = ((b[1] - c[1]) * (p[0] - c[0]) + (c[0] - b[0]) * (p[1] - c[1])) / d;
    let l2 = ((c[1] - a[1]) * (p[0] - c[0]) + (a[0] - c[0]) * (p[1] - c[1])) / d;
    let l3 = 1.0 - l1 - l2;
    (l1 >= -1e-6 && l2 >= -1e-6 && l3 >= -1e-6).then(|| l1 * a[2] + l2 * b[2] + l3 * c[2])
}

fn check(name: &str) {
    let Some(s) = scene(name) else { return };
    let tpl = track::graph::read_ptf_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("_test/extract_hillside/hillside.ptf")).unwrap();
    let c = acconv::convert(&s, &tpl, &acconv::Options::default()).unwrap();
    let nr = track::model::Track::from_graph(&c.graph).unwrap();
    let opt = acscenery::Options::default();
    let plan = acscenery::plan(&s, &nr, c.report.offset, c.report.height_base, &opt);
    let st = &plan.stats;
    eprintln!(
        "{name}: {} meshes as scenery ({} small), {} triangles, {} on NR's ground dropped, {} squares, {} textures",
        st.meshes, st.meshes_small, st.triangles, st.on_ground, plan.cells.len(), plan.textures.len()
    );
    assert!(!plan.cells.is_empty());
    assert!(plan.cells.len() <= opt.max_objects, "{} objects, squares {:.0} m", plan.cells.len(), st.cell_m);
    // NR2003 crashes with too much scenery (1.4 million crashed, 600 thousand loads)
    assert!(st.triangles <= opt.max_triangles, "{} triangles (from {}, merged within {:.2} m)", st.triangles, st.triangles_before, st.merge_m);
    eprintln!("  squares {:.0} m", st.cell_m);

    // within make3do's limits, every index in range, names unique
    let mut names = std::collections::HashSet::new();
    for cell in &plan.cells {
        assert!(names.insert(cell.name.to_ascii_lowercase()), "two squares named {}", cell.name);
        for sh in &cell.shapes {
            assert!(!sh.tris.is_empty());
            assert!(sh.tris.len() <= acscenery::MAX_SHAPE_FACES && sh.verts.len() <= acscenery::MAX_SHAPE_VERTS);
            assert!(sh.tris.iter().flatten().all(|&i| (i as usize) < sh.verts.len()));
        }
    }
    let mips: std::collections::HashSet<String> = plan.textures.iter().map(|t| t.mip.to_ascii_lowercase()).collect();
    assert_eq!(mips.len(), plan.textures.len(), "two textures share a .mip name");
    assert!(plan.textures.iter().all(|t| t.mip.len() <= 30));

    // the scenery's land meets NR2003's ground: just beyond the ground's outer edge, AC's land
    // (now scenery) should be at about the ground's height there
    let mut grid: std::collections::HashMap<(i32, i32), Vec<[[f64; 3]; 3]>> = Default::default();
    for cell in &plan.cells {
        for sh in &cell.shapes {
            for t in &sh.tris {
                let p: [[f64; 3]; 3] = std::array::from_fn(|k| {
                    let v = sh.verts[t[k] as usize];
                    [v[0] as f64 + cell.centre[0], v[1] as f64 + cell.centre[1], v[2] as f64 + cell.centre[2]]
                });
                let (lo, hi) = (
                    [p[0][0].min(p[1][0]).min(p[2][0]), p[0][1].min(p[1][1]).min(p[2][1])],
                    [p[0][0].max(p[1][0]).max(p[2][0]), p[0][1].max(p[1][1]).max(p[2][1])],
                );
                if hi[0] - lo[0] > 200.0 || hi[1] - lo[1] > 200.0 {
                    continue;
                }
                for i in (lo[0] / 10.0).floor() as i32..=(hi[0] / 10.0).floor() as i32 {
                    for j in (lo[1] / 10.0).floor() as i32..=(hi[1] / 10.0).floor() as i32 {
                        grid.entry((i, j)).or_default().push(p);
                    }
                }
            }
        }
    }
    let (mut gaps, mut tried) = (Vec::new(), 0);
    for seg in &nr.segments {
        for t in [0.25, 0.75] {
            for (d, out) in [(seg.max_dlat(t), 1.0), (seg.min_dlat(t), -1.0)] {
                tried += 1;
                let edge = seg.height(t, d);
                let p = seg.point(t, d + out * 1.0);
                let Some(tris) = grid.get(&((p[0] / 10.0).floor() as i32, (p[1] / 10.0).floor() as i32)) else { continue };
                let best = tris
                    .iter()
                    .filter_map(|q| height_in(p, q[0], q[1], q[2]))
                    .map(|z| z - edge)
                    .filter(|dz| dz.abs() < 5.0)
                    .min_by(|a, b| a.abs().total_cmp(&b.abs()));
                if let Some(dz) = best {
                    gaps.push(dz.abs());
                }
            }
        }
    }
    gaps.sort_by(|a, b| a.total_cmp(b));
    let median = gaps.get(gaps.len() / 2).copied().unwrap_or(f64::MAX);
    let p90 = gaps.get(gaps.len() * 9 / 10).copied().unwrap_or(f64::MAX);
    eprintln!("  land beside the ground's edge found at {} of {} points; height step median {:.2} m, 90% under {:.2} m", gaps.len(), tried, median, p90);
    assert!(gaps.len() * 3 > tried, "AC's land wasn't found beside NR2003's ground");
    assert!(median < 0.3, "the scenery doesn't line up with the track (median step {median:.2} m)");

    // the PAS text for a square is well formed (the counts make3do reads match)
    let cell = plan.cells.iter().max_by_key(|c| c.shapes.len()).unwrap();
    let text = acscenery::pas(cell, &plan.materials);
    assert!(text.starts_with("# Papyrus") && text.contains("VERSION\t200"));
    assert_eq!(text.matches("SHAPE {").count(), cell.shapes.len());
    let faces: usize = cell.shapes.iter().map(|s| s.tris.len()).sum();
    assert_eq!(text.matches("\t\t\tFACE ").count(), faces);
    assert_eq!(text.matches("VERTEX_NORMAL").count(), 3 * faces);
    let script = acscenery::psg(cell, opt.lod_m, opt.detail_lod_m);
    assert!(script.contains(&format!("OUTPUT")) && script.contains(&format!("\"{}.3do\"", cell.name)));
}

#[test]
fn mugello_scenery() {
    check("mugello");
}

#[test]
fn monaco_2020_scenery() {
    check("monaco_2020");
}
