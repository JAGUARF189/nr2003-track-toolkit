//! Convert an AC track with its scenery: the track (as acconvert), then AC's buildings, trees and
//! land as .3do objects with .mip textures, placed in the track.
//! cargo run --release --example acscenery -- <AC folder> <layout|-> <template folder> <out folder> <name>
//!     [--plan-only] [--tex N] [--cell M] [--lod M] [--keep-pas]
use std::path::PathBuf;
use std::time::Instant;
use ttk::{ac, acconv, acscenery, track};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    anyhow::ensure!(a.len() >= 6, "usage: acscenery <AC folder> <layout|-> <template folder> <out folder> <name> [--plan-only] [--tex N] [--cell M] [--lod M] [--keep-pas]");
    let flag = |f: &str| a.iter().any(|x| x == f);
    let val = |f: &str| a.iter().position(|x| x == f).and_then(|k| a.get(k + 1)).and_then(|v| v.parse::<f64>().ok());
    let mut opt = acscenery::Options::default();
    if let Some(v) = val("--tex") {
        opt.tex_side = v as u32;
    }
    if let Some(v) = val("--cell") {
        opt.cell_m = v;
    }
    if let Some(v) = val("--lod") {
        opt.lod_m = v;
    }
    opt.keep_pas = flag("--keep-pas");
    if let Some(v) = val("--budget") {
        opt.max_triangles = v as usize;
    }
    if let Some(v) = val("--object-tris") {
        opt.max_object_triangles = v as usize;
    }
    let out = PathBuf::from(&a[4]);
    std::fs::create_dir_all(&out)?;
    let t0 = Instant::now();
    let tex_dir = acscenery::texture_dir_for(&out);
    let _ = std::fs::remove_dir_all(&tex_dir);
    let scene_file = out.join("_ttk_ac.acscene");
    let summary = ac::export_scene_textures(&PathBuf::from(&a[1]), a[2].trim_start_matches('-'), &scene_file, 1024, &tex_dir)?;
    let scene = ac::AcScene::read(&scene_file, 0)?;
    let _ = std::fs::remove_file(&scene_file);
    println!("AC: {summary} ({:.1}s)", t0.elapsed().as_secs_f64());

    let tpl_dir = PathBuf::from(&a[3]);
    let tpl_file = track::graph::track_file_in(&tpl_dir).ok_or_else(|| anyhow::anyhow!("no template track in {}", tpl_dir.display()))?;
    let (bytes, _) = track::graph::load_ptf_bytes(&tpl_file)?;
    let template = track::graph::parse_ptf(&bytes)?;
    let mut c = acconv::convert(&scene, &template, &acconv::Options::default())?;
    println!("track: {} pieces, worst {:.2} m, closing gap {:.3} m ({:.1}s)", c.report.pieces, c.report.worst_m, c.report.closing_gap_m, t0.elapsed().as_secs_f64());

    let nr = track::model::Track::from_graph(&c.graph)?;
    let plan = acscenery::plan(&scene, &nr, c.report.offset, c.report.height_base, &opt);
    let st = &plan.stats;
    let biggest = plan.cells.iter().map(|c| c.triangles()).max().unwrap_or(0);
    let shapes: usize = plan.cells.iter().map(|c| c.shapes.len()).sum();
    println!(
        "scenery: {} meshes ({} small; {} track, {} hidden, {} 3D grass left out), {} triangles kept, {} on NR's ground dropped, {} degenerate; \
         {} squares, {} shapes, biggest square {} triangles; {} textures; from {} triangles, merged within {:.3} m ({:.1}s)",
        st.meshes, st.meshes_small, st.meshes_track, st.meshes_hidden, st.meshes_grass, st.triangles, st.on_ground, st.degenerate,
        plan.cells.len(), shapes, biggest, plan.textures.len(), st.triangles_before, st.merge_m, t0.elapsed().as_secs_f64()
    );
    if flag("--by-material") {
        let mut by: std::collections::HashMap<usize, usize> = Default::default();
        for cell in &plan.cells {
            for sh in &cell.shapes {
                *by.entry(sh.material).or_default() += sh.tris.len();
            }
        }
        let mut v: Vec<(usize, usize)> = by.into_iter().collect();
        v.sort_by(|x, y| y.1.cmp(&x.1));
        for (m, n) in v.iter().take(25) {
            let ac = scene.materials.get(*m);
            println!("  {:>8} {:<30} {:?} shader {}", n, plan.materials[*m].name, plan.materials[*m].alpha, ac.map_or("-", |a| a.shader.as_str()));
        }
    }
    if flag("--plan-only") {
        return Ok(());
    }
    let settings = ttk::config::Settings::default();
    let built = acscenery::build(&plan, &tex_dir, &out, &settings.sandbox_dir, &opt, &|_| {})?;
    println!("built: {} objects, {} textures, {} problems ({:.1}s)", built.objects.len(), built.mips, built.problems.len(), t0.elapsed().as_secs_f64());
    for p in built.problems.iter().take(10) {
        println!("  problem: {p}");
    }
    let placed = acscenery::place(&mut c.graph, &built)?;
    let sup = ttk::acsupport::finish(&mut c.graph, &scene, c.report.offset, &template, &tpl_dir, &out, &a[5])?;
    println!("support: {} pits and {} grid spots from AC ({} moved onto the ground), flaggers {:?} (missing {:?}), {} .lp files, lap {:.1} m", sup.pits_from_ac, sup.grid_from_ac, sup.moved_onto_ground, sup.flaggers, sup.flaggers_missing, sup.lp_files.len(), sup.lap_m);
    let w = acconv::write_track(&c, &out, &a[5], &tpl_dir)?;
    let _ = std::fs::remove_dir_all(&tex_dir);
    println!("placed {placed} objects; saved {} ({:.1}s)", w.ptf.display(), t0.elapsed().as_secs_f64());
    Ok(())
}
