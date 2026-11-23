//! Minimal TGA writer in the most conservative layout makemip accepts:
//! uncompressed true-color (type 2), bottom-left origin, 24-bit when the
//! image is fully opaque and 32-bit with 8 alpha bits otherwise.

use anyhow::{bail, Result};
use image::RgbaImage;
use std::path::Path;

/// Writes `img` to `path`. Returns true if an alpha channel was written.
pub fn write_tga(path: &Path, img: &RgbaImage) -> Result<bool> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || w > u16::MAX as u32 || h > u16::MAX as u32 {
        bail!("image size {w}x{h} can't be stored in a TGA");
    }
    let alpha = img.pixels().any(|p| p.0[3] != 255);
    let bpp: u8 = if alpha { 32 } else { 24 };

    let mut out = Vec::with_capacity(18 + (w as usize) * (h as usize) * (bpp as usize / 8));
    let mut hdr = [0u8; 18];
    hdr[2] = 2; // uncompressed true-color
    hdr[12..14].copy_from_slice(&(w as u16).to_le_bytes());
    hdr[14..16].copy_from_slice(&(h as u16).to_le_bytes());
    hdr[16] = bpp;
    hdr[17] = if alpha { 8 } else { 0 }; // alpha bits; origin bottom-left
    out.extend_from_slice(&hdr);

    for y in (0..h).rev() {
        for x in 0..w {
            let p = img.get_pixel(x, y).0;
            out.extend_from_slice(&[p[2], p[1], p[0]]);
            if alpha {
                out.push(p[3]);
            }
        }
    }
    std::fs::write(path, out)?;
    Ok(alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_through_image_crate() {
        let dir = std::env::temp_dir().join(format!("ttk_tga_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut img = RgbaImage::new(4, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(3, 1, image::Rgba([0, 0, 255, 128]));
        let p = dir.join("t.tga");
        assert!(write_tga(&p, &img).unwrap());
        let back = image::open(&p).unwrap().to_rgba8();
        assert_eq!(back.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(back.get_pixel(3, 1).0, [0, 0, 255, 128]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
