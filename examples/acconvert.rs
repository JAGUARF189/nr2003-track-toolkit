//! Convert an Assetto Corsa track to an NR2003 .ptf from the command line:
//! cargo run --release --example acconvert -- <AC track folder> <layout|-> <template track folder> <out folder> <name> [--fewer]
use std::path::PathBuf;
use ttk::{ac, acconv, track};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    anyhow::ensure!(a.len() >= 6, "usage: acconvert <AC track folder> <layout|-> <template track folder> <out folder> <name>");
    let (dir, layout, tpl_dir, out, name) = (PathBuf::from(&a[1]), a[2].trim_start_matches('-').to_string(), PathBuf::from(&a[3]), PathBuf::from(&a[4]), &a[5]);
    let tmp = std::env::temp_dir().join(format!("acconvert_{}.acscene", std::process::id()));
    ac::export_scene(&dir, &layout, &tmp, 0)?;
    let scene = ac::AcScene::read(&tmp, 0)?;
    let _ = std::fs::remove_file(&tmp);
    let tpl_file = track::graph::track_file_in(&tpl_dir).ok_or_else(|| anyhow::anyhow!("no .ptf or .dat in {}", tpl_dir.display()))?;
    let (bytes, _) = track::graph::load_ptf_bytes(&tpl_file)?;
    let template = track::graph::parse_ptf(&bytes)?;
    let opt = if a.iter().any(|x| x == "--fewer") { acconv::Options::fewer_pieces() } else { acconv::Options::default() };
    let mut c = acconv::convert(&scene, &template, &opt)?;
    println!("{:#?}", c.report);
    let sup = ttk::acsupport::finish(&mut c.graph, &scene, c.report.offset, &template, &tpl_dir, &out, &a[5])?;
    println!("support: {} pits and {} grid spots from AC ({} moved onto the ground), flaggers {:?} (missing {:?}), {} .lp files, lap {:.1} m", sup.pits_from_ac, sup.grid_from_ac, sup.moved_onto_ground, sup.flaggers, sup.flaggers_missing, sup.lp_files.len(), sup.lap_m);
    let w = acconv::write_track(&c, &out, name, &tpl_dir)?;
    println!("wrote {} ({} textures copied, missing {:?})", w.ptf.display(), w.copied.len(), w.missing);
    Ok(())
}
