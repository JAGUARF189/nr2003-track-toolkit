//! Time loading a track's textures the way the 3D view does:
//! cargo run --release --example texload -- <track.ptf|.dat>
use std::time::Instant;
use ttk::track::{graph, mesh, model::Track, textures::TexStore};

fn main() {
    let p = std::path::PathBuf::from(std::env::args().nth(1).expect("track file"));
    let t0 = Instant::now();
    let (bytes, _) = graph::load_ptf_bytes(&p).unwrap();
    let g = graph::parse_ptf(&bytes).unwrap();
    let t = Track::from_graph(&g).unwrap();
    let m = mesh::build(&t, true);
    let t1 = Instant::now();
    let store = TexStore::new(p.parent().unwrap());
    let files = m.textures();
    let mut found = 0;
    let mut bytes = 0usize;
    let mut missing = Vec::new();
    for f in &files {
        match store.load(f, 256) {
            Some(i) => {
                found += 1;
                bytes += i.as_raw().len();
            }
            None => missing.push(f.clone()),
        }
    }
    println!(
        "track+mesh {:.2}s ({} batches), textures {:.2}s: {found}/{} found, {:.1} MB; missing {:?}",
        (t1 - t0).as_secs_f64(),
        m.batches.len(),
        t1.elapsed().as_secs_f64(),
        files.len(),
        bytes as f64 / 1e6,
        &missing[..missing.len().min(5)]
    );
}
