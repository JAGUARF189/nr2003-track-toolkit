//! Top-down picture comparing an AC track with its NR2003 conversion (drawn in software, so it
//! works without a screen): AC road edges in red, NR2003 road in grey with white edges, grass
//! green, walls yellow, piece joins as small ticks.
//! cargo run --release --example acplot -- <converted .ptf> <AC track folder> <layout|-> <out.png> [zoom metres] [centre x,y]
use image::{Rgb, RgbImage};
use std::path::PathBuf;
use ttk::track::model::Track;
use ttk::{ac, acconv, track};

struct Canvas {
    img: RgbImage,
    cx: f64,
    cy: f64,
    scale: f64,
}

impl Canvas {
    fn px(&self, x: f64, y: f64) -> (f64, f64) {
        let (w, h) = (self.img.width() as f64, self.img.height() as f64);
        (w / 2.0 + (x - self.cx) * self.scale, h / 2.0 - (y - self.cy) * self.scale)
    }
    fn dot(&mut self, x: f64, y: f64, c: Rgb<u8>) {
        if x >= 0.0 && y >= 0.0 && (x as u32) < self.img.width() && (y as u32) < self.img.height() {
            self.img.put_pixel(x as u32, y as u32, c);
        }
    }
    fn line(&mut self, a: [f64; 2], b: [f64; 2], c: Rgb<u8>) {
        let (ax, ay) = self.px(a[0], a[1]);
        let (bx, by) = self.px(b[0], b[1]);
        let n = ((bx - ax).abs().max((by - ay).abs()).ceil() as usize).clamp(1, 20000);
        for k in 0..=n {
            let t = k as f64 / n as f64;
            self.dot(ax + (bx - ax) * t, ay + (by - ay) * t, c);
        }
    }
    /// Fill the quad a-b-c-d (convex) roughly by drawing many lines.
    fn fill(&mut self, a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2], col: Rgb<u8>) {
        let (pa, pd) = (self.px(a[0], a[1]), self.px(d[0], d[1]));
        let steps = ((pa.0 - pd.0).abs().max((pa.1 - pd.1).abs()).ceil() as usize).clamp(1, 4000) * 2;
        for k in 0..=steps {
            let t = k as f64 / steps as f64;
            let p = [a[0] + (d[0] - a[0]) * t, a[1] + (d[1] - a[1]) * t];
            let q = [b[0] + (c[0] - b[0]) * t, b[1] + (c[1] - b[1]) * t];
            self.line(p, q, col);
        }
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    anyhow::ensure!(a.len() >= 5, "usage: acplot <converted .ptf> <AC track folder> <layout|-> <out.png> [zoom metres] [centre x,y]");
    let g = track::graph::read_ptf_file(&PathBuf::from(&a[1]))?;
    let t = Track::from_graph(&g)?;
    let tmp = std::env::temp_dir().join(format!("acplot_{}.acscene", std::process::id()));
    ac::export_scene(&PathBuf::from(&a[2]), a[3].trim_start_matches('-'), &tmp, 0)?;
    let scene = ac::AcScene::read(&tmp, 0)?;
    let _ = std::fs::remove_file(&tmp);
    let cl = acconv::centerline(&scene.ai, 1.0)?;
    // the conversion centred the track: find that offset from the AC centerline's box
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in &cl.pos {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let off = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
    let size = 1600u32;
    let zoom: Option<f64> = a.get(5).and_then(|z| z.parse().ok());
    let centre: Option<[f64; 2]> = a.get(6).and_then(|c| {
        let v: Vec<f64> = c.split(',').filter_map(|x| x.parse().ok()).collect();
        (v.len() == 2).then(|| [v[0], v[1]])
    });
    let span = zoom.unwrap_or((hi[0] - lo[0]).max(hi[1] - lo[1]) + 80.0);
    let c0 = centre.unwrap_or([0.0, 0.0]);
    let mut cv = Canvas { img: RgbImage::from_pixel(size, size, Rgb([24, 28, 32])), cx: c0[0], cy: c0[1], scale: size as f64 / span };
    // NR2003 strips and walls
    for s in &t.segments {
        let n = ((s.length / 2.0).ceil() as usize).max(1);
        for (a0, b0, surf) in s.strips(0.5) {
            let col = match surf {
                4 => Rgb([46, 90, 40]),
                1 => Rgb([90, 90, 96]),
                _ => Rgb([120, 100, 140]),
            };
            let f = s.fs.iter().find(|f| (0.5 * (f.dlat[0] + f.dlat[1]) - a0).abs() < 1e-6);
            let _ = (f, b0);
            for k in 0..n {
                let (t0, t1) = (k as f64 / n as f64, (k + 1) as f64 / n as f64);
                let strip = |t: f64| {
                    let st = s.strips(t);
                    st.into_iter().filter(|x| x.2 == surf).min_by(|x, y| (x.0 - a0).abs().total_cmp(&(y.0 - a0).abs())).map(|x| (x.0, x.1))
                };
                if let (Some(p), Some(q)) = (strip(t0), strip(t1)) {
                    cv.fill(s.point(t0, p.0), s.point(t1, q.0), s.point(t1, q.1), s.point(t0, p.1), col);
                }
            }
        }
        for w in &s.ws {
            let mut prev = s.point(0.0, w.dlat[0]);
            for k in 1..=n {
                let t = k as f64 / n as f64;
                let p = s.point(t, w.dlat[0] + (w.dlat[1] - w.dlat[0]) * t);
                cv.line(prev, p, Rgb([230, 200, 60]));
                prev = p;
            }
        }
        // road edges in white, from the asphalt strip
        if let Some(f) = s.fs.iter().find(|f| f.surface == 1) {
            let right = |t: f64| f.dlat[0] + (f.dlat[1] - f.dlat[0]) * t;
            let left = |t: f64| s.fs.iter().map(|x| x.dlat[0] + (x.dlat[1] - x.dlat[0]) * t).filter(|&d| d > right(t) + 1e-6).fold(f64::MAX, f64::min);
            let mut pr = s.point(0.0, right(0.0));
            let mut pl = s.point(0.0, left(0.0));
            for k in 1..=n {
                let t = k as f64 / n as f64;
                let (r, l) = (s.point(t, right(t)), s.point(t, left(t)));
                cv.line(pr, r, Rgb([240, 240, 240]));
                cv.line(pl, l, Rgb([240, 240, 240]));
                pr = r;
                pl = l;
            }
        }
        // joins between pieces: a short tick across the road
        let h = s.start[2];
        let (lx, ly) = (-h.sin(), h.cos());
        let p = [s.start[0], s.start[1]];
        cv.line([p[0] - lx * 3.0, p[1] - ly * 3.0], [p[0] + lx * 3.0, p[1] + ly * 3.0], Rgb([255, 140, 40]));
    }
    // AC road edges (from the AI line's left/right distances), in red
    let n = scene.ai.len();
    let mut prev: Option<([f64; 2], [f64; 2])> = None;
    for k in 0..=n {
        let p = scene.ai[k % n];
        let q = scene.ai[(k + 1) % n];
        let (dx, dy) = ((q.pos[0] - p.pos[0]) as f64, (q.pos[1] - p.pos[1]) as f64);
        let len = dx.hypot(dy).max(1e-9);
        let (lx, ly) = (-dy / len, dx / len);
        let (x, y) = (p.pos[0] as f64 - off[0], p.pos[1] as f64 - off[1]);
        let l = [x + lx * p.left as f64, y + ly * p.left as f64];
        let r = [x - lx * p.right as f64, y - ly * p.right as f64];
        if let Some((pl, pr)) = prev {
            cv.line(pl, l, Rgb([235, 60, 50]));
            cv.line(pr, r, Rgb([235, 60, 50]));
        }
        prev = Some((l, r));
    }
    cv.img.save(&a[4])?;
    println!("saved {} ({} pieces, view {:.0} m across)", a[4], t.segments.len(), span);
    Ok(())
}
