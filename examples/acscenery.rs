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
    opt.ground_texture = acscenery::grass_texture(&c.graph).map(|f| (tpl_dir.clone(), f));
    let plan = acscenery::plan(&scene, &nr, c.report.offset, c.report.height_base, &opt);
    let st = &plan.stats;
    let biggest = plan.cells.iter().map(|c| c.triangles()).max().unwrap_or(0);
    let shapes: usize = plan.cells.iter().map(|c| c.shapes.len()).sum();
    println!(
        "scenery: {} meshes ({} small; {} track, {} hidden, {} 3D grass left out), {} triangles kept, {} on NR's ground dropped, {} degenerate; \
         {} squares, {} shapes, biggest square {} triangles; {} textures; from {} triangles, merged within {:.3} m, {} meshes left out ({:.1}s)",
        st.meshes, st.meshes_small, st.meshes_track, st.meshes_hidden, st.meshes_grass, st.triangles, st.on_ground, st.degenerate,
        plan.cells.len(), shapes, biggest, plan.textures.len(), st.triangles_before, st.merge_m, st.meshes_left_out, t0.elapsed().as_secs_f64()
    );
    if flag("--multilayer") {
        for m in scene.materials.iter().filter(|m| m.detail.is_some()) {
            let t = |i: Option<usize>| i.and_then(|i| scene.textures.get(i)).map_or("-".to_string(), |t| t.name.clone());
            println!("  multi-layer {:<28} {:<24} {} -> {} x{}", m.name, m.shader, t(m.texture), t(m.detail), m.detail_mult);
        }
    }
    if flag("--uvspan") {
        for m in scene.meshes.iter().filter(|m| m.material.and_then(|k| scene.materials.get(k)).is_some_and(|mt| mt.detail.is_some() || mt.colour_map_only)).take(12) {
            let (mut lo, mut hi, mut ulo, mut uhi) = ([f32::MAX; 2], [f32::MIN; 2], [f32::MAX; 2], [f32::MIN; 2]);
            for v in m.verts.chunks_exact(8) {
                for c in 0..2 {
                    lo[c] = lo[c].min(v[c]);
                    hi[c] = hi[c].max(v[c]);
                    ulo[c] = ulo[c].min(v[6 + c]);
                    uhi[c] = uhi[c].max(v[6 + c]);
                }
            }
            let mt = &scene.materials[m.material.unwrap()];
            println!("  uvspan {:<24} {:<14} size {:.0} x {:.0} m, uv span {:.2} x {:.2} -> {:.2} m per uv unit, detail x{}", m.name, mt.name, hi[0] - lo[0], hi[1] - lo[1], uhi[0] - ulo[0], uhi[1] - ulo[1], ((hi[0] - lo[0]).max(hi[1] - lo[1])) / ((uhi[0] - ulo[0]).max(uhi[1] - ulo[1])).max(1e-6), mt.detail_mult);
        }
    }
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
