//! Print the checker's findings for a .ptf: cargo run --release --example checktrack -- <file.ptf>
fn main() {
    let p = std::env::args().nth(1).expect("usage: checktrack <file.ptf>");
    let g = ttk::track::graph::read_ptf_file(std::path::Path::new(&p)).unwrap();
    let t = ttk::track::model::Track::from_graph(&g).unwrap();
    for pr in ttk::track::check::check(&t) {
        println!("{:?}\t{}", pr.level, pr.text);
    }
}
