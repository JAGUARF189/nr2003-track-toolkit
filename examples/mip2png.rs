//! Decode .mip textures to PNG pictures (largest level), drawn over a checkerboard so see-through
//! parts show: cargo run --release --example mip2png -- <out folder> <file.mip>...
use std::path::PathBuf;
use ttk::mip::Mip;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    anyhow::ensure!(a.len() >= 3, "usage: mip2png <out folder> <file.mip>...");
    let out = PathBuf::from(&a[1]);
    std::fs::create_dir_all(&out)?;
    for f in &a[2..] {
        let m = Mip::open(&PathBuf::from(f))?;
        let img = m.decode_largest()?;
        let mut shown = image::RgbaImage::new(img.width(), img.height());
        for (x, y, p) in img.enumerate_pixels() {
            let check = if ((x / 8) + (y / 8)) % 2 == 0 { 200.0 } else { 120.0 };
            let al = p.0[3] as f32 / 255.0;
            let mix = |c: u8| (c as f32 * al + check * (1.0 - al)) as u8;
            shown.put_pixel(x, y, image::Rgba([mix(p.0[0]), mix(p.0[1]), mix(p.0[2]), 255]));
        }
        let name = PathBuf::from(f).file_stem().unwrap().to_string_lossy().to_string();
        shown.save(out.join(format!("{name}.png")))?;
        println!("{name}: {}x{}, type {} ({}), {} levels", m.info.width, m.info.height, m.info.kind, ttk::mip::type_name(m.info.kind), m.levels.len());
    }
    Ok(())
}
