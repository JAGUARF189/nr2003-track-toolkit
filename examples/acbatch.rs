//! Convert every AC track (and layout) in a folder, to find tracks the converter can't handle.
//! Each result is saved to a temporary folder (which reads it back and compares) and deleted.
//! Writes one line per layout to <log> as it goes.
//! cargo run --release --example acbatch -- <AC content\tracks folder> <template track folder> <log file> [first N]
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;
use ttk::{ac, acconv, track};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    anyhow::ensure!(a.len() >= 4, "usage: acbatch <AC tracks folder> <template track folder> <log file> [first N]");
    let tracks = ac::list_tracks(&PathBuf::from(&a[1]));
    let limit: usize = a.get(4).and_then(|x| x.parse().ok()).filter(|_| a.get(4).is_some_and(|x| x != "--from")).unwrap_or(usize::MAX);
    let tpl_dir = PathBuf::from(&a[2]);
    let tpl_file = track::graph::track_file_in(&tpl_dir).ok_or_else(|| anyhow::anyhow!("no template track in {}", tpl_dir.display()))?;
    let (bytes, _) = track::graph::load_ptf_bytes(&tpl_file)?;
    let template = track::graph::parse_ptf(&bytes)?;
    // --from <name>: resume at that track, adding to the log
    let from = a.iter().position(|x| x == "--from").and_then(|k| a.get(k + 1)).cloned();
    let tracks: Vec<(String, PathBuf)> = match &from {
        Some(f) => tracks.iter().skip_while(|t| !t.0.eq_ignore_ascii_case(f)).cloned().collect(),
        None => tracks,
    };
    let mut log = std::fs::OpenOptions::new().create(true).append(true).open(&a[3])?;
    if from.is_none() {
        log.set_len(0)?;
        writeln!(log, "track\tlayout\tresult\tseconds\tpieces\tworst_m\tmean_m\theights_found\tac_ground\tac_walls\twarnings\tdetail")?;
    }
    let tmp_scene = std::env::temp_dir().join(format!("acbatch_{}.acscene", std::process::id()));
    let tmp_out = std::env::temp_dir().join(format!("acbatch_{}_out", std::process::id()));
    let (mut ok, mut failed) = (0, 0);
    for (name, dir) in tracks.iter().take(limit) {
        let info = match ac::info(dir) {
            Ok(i) => i,
            Err(e) => {
                writeln!(log, "{name}\t\tinfo-error\t0\t\t\t\t\t\t\t\t{e:#}")?;
                failed += 1;
                continue;
            }
        };
        for l in &info.layouts {
            let t0 = Instant::now();
            let r = (|| -> anyhow::Result<acconv::Report> {
                if l.ai_points == 0 {
                    anyhow::bail!("no AI line");
                }
                ac::export_scene(dir, &l.name, &tmp_scene, 0)?;
                let scene = ac::AcScene::read(&tmp_scene, 0)?;
                let c = acconv::convert(&scene, &template, &acconv::Options::default())?;
                let _ = std::fs::remove_dir_all(&tmp_out);
                acconv::write_track(&c, &tmp_out, "batch", &tpl_dir)?;
                Ok(c.report)
            })();
            let secs = t0.elapsed().as_secs_f64();
            match r {
                Ok(r) => {
                    let detail = if r.crashes.is_empty() { String::new() } else { format!("WON'T LOAD: {}", r.crashes.join(" / ")) };
                    writeln!(
                        log,
                        "{name}\t{}\t{}\t{secs:.1}\t{}\t{:.2}\t{:.3}\t{:.2}\t{}\t{}\t{}\t{detail}",
                        l.name,
                        if r.crashes.is_empty() { "ok" } else { "crash" },
                        r.pieces,
                        r.worst_m,
                        r.mean_m,
                        r.heights_found,
                        r.ac_ground_pieces,
                        r.ac_walls,
                        r.warnings
                    )?;
                    if r.crashes.is_empty() {
                        ok += 1;
                    } else {
                        failed += 1;
                    }
                }
                Err(e) => {
                    writeln!(log, "{name}\t{}\terror\t{secs:.1}\t\t\t\t\t\t\t\t{}", l.name, format!("{e:#}").replace(['\t', '\n'], " "))?;
                    failed += 1;
                }
            }
            log.flush()?;
        }
    }
    let _ = std::fs::remove_file(&tmp_scene);
    let _ = std::fs::remove_dir_all(&tmp_out);
    println!("{ok} converted, {failed} failed or won't load");
    Ok(())
}
