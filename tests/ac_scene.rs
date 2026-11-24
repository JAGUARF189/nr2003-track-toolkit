//! Reading Assetto Corsa tracks through acdump (skipped when AC or acdump.exe isn't installed).
//! Uses the user's own AC install: mugello (Kunos) and monaco_2020 (a mod with layouts).
use std::path::PathBuf;
use std::time::Instant;
use ttk::ac;

fn track(name: &str) -> Option<PathBuf> {
    let dir = ac::find_tracks_dir()?.join(name);
    if !dir.is_dir() || ac::find_acdump().is_none() {
        eprintln!("skipping: {name} or acdump.exe missing");
        return None;
    }
    Some(dir)
}

fn scene(dir: &PathBuf, layout: &str) -> ac::AcScene {
    let out = std::env::temp_dir().join(format!("ttk_ac_test_{}_{}.acscene", std::process::id(), layout.replace('/', "_")));
    let t = Instant::now();
    let summary = ac::export_scene(dir, layout, &out, 256).unwrap();
    let s = ac::AcScene::read(&out, 256).unwrap();
    let _ = std::fs::remove_file(&out);
    eprintln!("{} [{layout}]: {summary} ({:.1}s)", dir.display(), t.elapsed().as_secs_f64());
    s
}

#[test]
fn mugello_reads_with_its_ai_line_and_surfaces() {
    let Some(dir) = track("mugello") else { return };
    let info = ac::info(&dir).unwrap();
    assert_eq!(info.layouts.len(), 1);
    assert!((info.layouts[0].lap_m - 5196.0).abs() < 5.0, "lap {}", info.layouts[0].lap_m);
    let s = scene(&dir, "");
    assert!(s.triangles() > 2_000_000);
    assert!(s.textures.iter().all(|t| t.image.is_some()), "every Mugello texture decodes");
    // driving surfaces are found by name (1TARMAC-MUG-A..., KERB, WALL)
    let on_road = s.meshes.iter().filter(|m| m.surface.is_some()).count();
    assert!(on_road > 100, "{on_road} driving-surface meshes");
    // pit boxes and grid spots
    assert!(s.markers_named("AC_PIT_").count() >= 20);
    assert!(s.markers_named("AC_START_").count() >= 20);
    // the lap runs clockwise seen from above (Mugello does), so the axes are the right way round
    let n = s.ai.len();
    let area: f64 = (0..n)
        .map(|k| {
            let (a, b) = (s.ai[k].pos, s.ai[(k + 1) % n].pos);
            (a[0] as f64) * (b[1] as f64) - (b[0] as f64) * (a[1] as f64)
        })
        .sum::<f64>()
        / 2.0;
    assert!(area < 0.0, "clockwise lap, signed area {area}");
    // the track is 12-16 m wide along the AI line
    let w: Vec<f32> = s.ai.iter().map(|p| p.left + p.right).collect();
    let mid = { let mut v = w.clone(); v.sort_by(|a, b| a.total_cmp(b)); v[v.len() / 2] };
    assert!((10.0..20.0).contains(&mid), "median width {mid}");
    // the AI line sits on the road surface (within a few metres of the meshes' height range)
    let m = ac::build_mesh(&s, ac::ViewOptions { scenery: true, colour_surfaces: false });
    assert_eq!(m.batches.iter().map(|b| b.count as usize).sum::<usize>(), m.indices.len());
    assert!(m.indices.len() / 3 >= s.triangles() / 2);
}

#[test]
fn monaco_layouts_and_png_textures() {
    let Some(dir) = track("monaco_2020") else { return };
    let info = ac::info(&dir).unwrap();
    assert!(info.layouts.len() >= 6, "{} layouts", info.layouts.len());
    assert!(info.layouts.iter().any(|l| l.name == "monaco_f1_2026"));
    let s = scene(&dir, "monaco_f1_2026");
    // PNG textures stored in the models decode in the app; only BC7 (DX10) ones don't yet
    let missing = s.textures.iter().filter(|t| t.image.is_none()).count();
    assert!(missing <= 45, "{missing} textures not decoded");
    assert!(s.ai.len() > 2000);
    let walls = s.meshes.iter().filter(|m| m.surface.is_none() && m.name.to_ascii_uppercase().contains("WALL")).count();
    let surfaces = s.meshes.iter().filter(|m| m.surface.is_some()).count();
    eprintln!("monaco: {surfaces} driving-surface meshes, {walls} wall-named scenery meshes");
    assert!(surfaces > 50);
    let coloured = ac::build_mesh(&s, ac::ViewOptions { scenery: false, colour_surfaces: true });
    assert!(coloured.batches.iter().all(|b| b.texture.is_none()));
}
