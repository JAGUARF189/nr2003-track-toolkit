//! List the ground strips of a track's pieces around a distance along the lap: sideways extent,
//! surface, texture and its coordinates (how many times it repeats along and across).
//! cargo run --release --example stripdump -- <track.ptf or folder> <dlong m> [pieces either side]
use std::path::PathBuf;
use ttk::track::{graph, model::{surface_name, Track}};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let src = PathBuf::from(&a[1]);
    let file = if src.is_dir() { graph::track_file_in(&src).unwrap() } else { src };
    let (bytes, _) = graph::load_ptf_bytes(&file)?;
    let t = Track::from_graph(&graph::parse_ptf(&bytes)?)?;
    let at: f64 = a[2].parse()?;
    let around: usize = a.get(3).and_then(|x| x.parse().ok()).unwrap_or(1);
    let mut s0 = 0.0;
    let mut hit = 0;
    for (k, s) in t.segments.iter().enumerate() {
        if s0 + s.length >= at {
            hit = k;
            break;
        }
        s0 += s.length;
    }
    let n = t.segments.len();
    for k in (hit + n - around)..=(hit + n + around) {
        let s = &t.segments[k % n];
        println!("piece {} ({:.1} m, {})", k % n + 1, s.length, if s.curve { format!("curve r {:.0}", s.radius) } else { "straight".into() });
        let strips = s.strips(0.0);
        for (i, f) in s.fs.iter().enumerate() {
            let tex = f.tex.as_ref().map_or("-".to_string(), |t| {
                let along = (t.uv[2][1] - t.uv[1][1]).abs().max((t.uv[2][0] - t.uv[1][0]).abs());
                let across = (t.uv[1][0] - t.uv[0][0]).abs().max((t.uv[1][1] - t.uv[0][1]).abs());
                format!("{} uv {:?} (about {:.2} across, {:.2} along)", t.file, t.uv.map(|p| [(p[0] * 100.0).round() / 100.0, (p[1] * 100.0).round() / 100.0]), across, along)
            });
            let _ = &strips;
            println!("  F {i}: dlat {:.1}..{:.1} {} {}", f.dlat[0], f.dlat[1], surface_name(f.surface), tex);
        }
    }
    Ok(())
}
