//! Apply sample shape edits to a .ptf and save the result (for checking the viewer without
//! clicking): cargo run --release --example editdemo -- <in.ptf> <out.ptf>
use ttk::track::edit::{self, Shape};
use ttk::track::{graph, model::Track, save};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut g = graph::read_ptf_file(std::path::Path::new(&a[1])).unwrap();
    let t = Track::from_graph(&g).unwrap();
    let mut c = edit::reshape(&t, 14, Shape::Straight { length: t.segments[14].length + 10.0 }, true).unwrap();
    c.extend(edit::set_banking(&t, 7, 20.0, 18.0).unwrap());
    edit::apply(&mut g, &c).unwrap();
    let saved = save::save_graph(&g, std::path::Path::new(&a[2])).unwrap();
    println!("{} changes saved to {}", c.len(), saved.path.display());
}
