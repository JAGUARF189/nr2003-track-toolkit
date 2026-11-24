//! What AC surfaces lie across the road: at every `every`-th AI point, list the surface runs from
//! 30 m right to 30 m left of the AI line (name and width), and the walls found.
//! cargo run --release --example acprobe -- <AC track folder> <layout|-> [every]
use std::path::PathBuf;
use ttk::{ac, acconv};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    anyhow::ensure!(a.len() >= 3, "usage: acprobe <AC track folder> <layout|-> [every]");
    let every: usize = a.get(3).and_then(|x| x.parse().ok()).unwrap_or(250);
    let tmp = std::env::temp_dir().join(format!("acprobe_{}.acscene", std::process::id()));
    ac::export_scene(&PathBuf::from(&a[1]), a[2].trim_start_matches('-'), &tmp, 0)?;
    let s = ac::AcScene::read(&tmp, 0)?;
    let _ = std::fs::remove_file(&tmp);
    let roads = acconv::RoadHeights::new(&s);
    let walls = acconv::AcWalls::new(&s);
    let n = s.ai.len();
    for i in (0..n).step_by(every) {
        let p = s.ai[i];
        let q = s.ai[(i + 1) % n];
        let (dx, dy) = ((q.pos[0] - p.pos[0]) as f64, (q.pos[1] - p.pos[1]) as f64);
        let len = dx.hypot(dy).max(1e-9);
        let (lx, ly) = (-dy / len, dx / len);
        let mut runs: Vec<(String, f64, f64)> = Vec::new();
        let mut d = -30.0;
        while d <= 30.0 {
            let (x, y) = (p.pos[0] as f64 + lx * d, p.pos[1] as f64 + ly * d);
            let name = match roads.at_surface(x, y, p.pos[2] as f64, 4.0) {
                Some((_, k)) => s.surfaces[k].key.clone(),
                None => "-".into(),
            };
            match runs.last_mut() {
                Some(r) if r.0 == name => r.2 = d,
                _ => runs.push((name, d, d)),
            }
            d += 0.25;
        }
        let w = |sign: f64| walls.cast([p.pos[0] as f64, p.pos[1] as f64], [lx * sign, ly * sign], 40.0).map(|w| format!("{:.1}", w.0)).unwrap_or("-".into());
        println!(
            "AI {i} (edges L {:.1} R {:.1}; walls R {} L {}): {}",
            p.left,
            p.right,
            w(-1.0),
            w(1.0),
            runs.iter().map(|r| format!("{} {:.1}..{:.1}", r.0, r.1, r.2)).collect::<Vec<_>>().join(" | ")
        );
    }
    Ok(())
}
