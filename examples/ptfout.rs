//! Write a track's .ptf to a file, taking it out of the track's .dat if that's where it is:
//! cargo run --release --example ptfout -- <track folder or .ptf/.dat> <out.ptf>
use std::path::PathBuf;
use ttk::track::graph;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    anyhow::ensure!(a.len() >= 3, "usage: ptfout <track folder or .ptf/.dat> <out.ptf>");
    let src = PathBuf::from(&a[1]);
    let file = if src.is_dir() { graph::track_file_in(&src).ok_or_else(|| anyhow::anyhow!("no track in {}", src.display()))? } else { src };
    let (bytes, _) = graph::load_ptf_bytes(&file)?;
    std::fs::write(&a[2], &bytes)?;
    println!("{} -> {} ({} bytes)", file.display(), a[2], bytes.len());
    Ok(())
}
