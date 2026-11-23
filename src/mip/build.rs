//! Turn a picture into a .mip with Papyrus's makemip, either matching an
//! existing mip's settings ("replace") or with sensible defaults ("new").
//!
//! makemip flag ↔ header field (verified):
//!   (default) mapping 3 · -tile 0 · -tileV 1 · -tileU 2
//!   -priority n  -> header priority float n (0..4, decimals OK: 0.4 = CD CC CC 3E like stock)
//!   -single      -> 1 level
//!   (default) 16-bit (type 3/5) · -trueColor (6/7) · -dxt (10/11) · -trans 1-bit (4)

use super::{Mip, MipInfo, Transparency};
use crate::config::Resize;
use crate::logger::Logger;
use crate::runner::run_tool;
use crate::tga::write_tga;
use crate::util;
use anyhow::{bail, Context, Result};
use image::imageops::FilterType;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// 16-bit (types 3, 4, 5)
    Standard,
    /// 24/32-bit (types 6, 7)
    BestQuality,
    /// DXT (types 10, 11) — what NR2003 uses for almost everything
    Compressed,
}

impl Family {
    pub fn of_kind(kind: u8) -> Self {
        match kind {
            3..=5 => Family::Standard,
            6 | 7 => Family::BestQuality,
            _ => Family::Compressed,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Recipe {
    pub family: Family,
    pub mapping: u8,
    pub priority: f32,
    pub single_level: bool,
    /// Exact output size; `None` = nearest power of two to the picture's size.
    pub size: Option<(u32, u32)>,
    /// The original was a 1-bit cut-out (type 4): keep using `-trans` for cut-out pictures.
    pub cutout_type: bool,
    /// Type of the mip being replaced (for explaining changes).
    pub original_kind: Option<u8>,
}

impl Recipe {
    /// Same settings as an existing mip.
    pub fn matching(m: &Mip) -> Self {
        Self {
            family: Family::of_kind(m.info.kind),
            mapping: m.info.mapping,
            priority: m.info.priority,
            single_level: m.levels.len() == 1,
            size: Some((m.info.width, m.info.height)),
            cutout_type: m.info.kind == 4,
            original_kind: Some(m.info.kind),
        }
    }

    /// Defaults for a brand-new texture, like the stock files: track surfaces
    /// (trackmat) tile and use priority 0.4; everything else stretches with 0.1.
    pub fn new_texture(track_surface: bool, best_quality: bool) -> Self {
        Self {
            family: if best_quality {
                Family::BestQuality
            } else {
                Family::Compressed
            },
            mapping: if track_surface { 0 } else { 3 },
            priority: if track_surface { 0.4 } else { 0.1 },
            single_level: false,
            size: None,
            cutout_type: false,
            original_kind: None,
        }
    }

    fn makemip_flags(&self, picture_alpha: Transparency) -> Vec<String> {
        let mut f = Vec::new();
        match self.family {
            Family::Standard => {}
            Family::BestQuality => f.push("-trueColor".into()),
            Family::Compressed => f.push("-dxt".into()),
        }
        if self.cutout_type
            && picture_alpha == Transparency::Holes
            && self.family == Family::Standard
        {
            f.push("-trans".into());
        }
        match self.mapping {
            0 => f.push("-tile".into()),
            1 => f.push("-tileV".into()),
            2 => f.push("-tileU".into()),
            _ => {}
        }
        f.push("-priority".into());
        f.push(format!(
            "{}",
            (self.priority.clamp(0.0, 4.0) * 1000.0).round() / 1000.0
        ));
        if self.single_level {
            f.push("-single".into());
        }
        f
    }
}

pub struct Built {
    pub target: PathBuf,
    pub info: MipInfo,
    /// Previous file, if one was replaced.
    pub backup: Option<PathBuf>,
    /// Plain-English notes for the user.
    pub notes: Vec<String>,
}

/// Mip-friendly file name: spaces and odd characters become '_'.
pub fn safe_stem(stem: &str) -> String {
    let s: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "texture".into()
    } else {
        s
    }
}

/// File types the Textures tab shows.
pub fn is_texture_ext(ext: &str) -> bool {
    matches!(ext, "mip" | "stp")
}

pub fn is_picture(p: &Path) -> bool {
    matches!(
        util::ext_lower(p).as_str(),
        "png" | "jpg" | "jpeg" | "bmp" | "tga"
    )
}

/// Build `target` from `picture`. `work` is a scratch folder. If `target`
/// exists it is backed up to `<its folder>\_ttk_backup\` first.
pub fn build(
    sandbox_dir: &Path,
    picture: &Path,
    target: &Path,
    recipe: &Recipe,
    work: &Path,
) -> Result<Built> {
    let mut notes = Vec::new();
    let img = image::open(picture)
        .with_context(|| format!("can't open {} as a picture", util::file_name(picture)))?
        .to_rgba8();
    let source_alpha = Transparency::of(&img);
    match (recipe.original_kind, source_alpha) {
        (Some(4 | 5 | 7 | 11), Transparency::None) => notes.push(
            "Your picture has no see-through parts, so the new texture is solid all over.".into(),
        ),
        (Some(3 | 6), Transparency::Holes | Transparency::Soft) => {
            notes.push("Your picture has see-through parts, so the new texture does too.".into())
        }
        _ => {}
    }
    let (w, h) = img.dimensions();
    // .stp menu images are any size and are made with makestp instead.
    let stp = util::ext_lower(target) == "stp";

    let (tw, th) = match recipe.size {
        Some(s) => s,
        None if stp => (w.clamp(1, 4096), h.clamp(1, 4096)),
        None => (
            Resize::Nearest.apply(w).unwrap_or(w).clamp(1, 4096),
            Resize::Nearest.apply(h).unwrap_or(h).clamp(1, 4096),
        ),
    };
    let mut img = if (tw, th) != (w, h) {
        let src_aspect = w as f32 / h as f32;
        let dst_aspect = tw as f32 / th as f32;
        if (src_aspect / dst_aspect - 1.0).abs() > 0.05 {
            notes.push(format!(
                "Your picture ({w}×{h}) was stretched to {tw}×{th} to fit."
            ));
        } else {
            notes.push(format!(
                "Your picture was resized from {w}×{h} to {tw}×{th}."
            ));
        }
        image::imageops::resize(&img, tw, th, FilterType::Lanczos3)
    } else {
        img
    };
    // Resizing blurs alpha edges; keep the picture's own kind of transparency.
    match source_alpha {
        Transparency::None => img.pixels_mut().for_each(|p| p.0[3] = 255),
        Transparency::Holes => img
            .pixels_mut()
            .for_each(|p| p.0[3] = if p.0[3] >= 128 { 255 } else { 0 }),
        Transparency::Soft => {}
    }

    std::fs::create_dir_all(work)?;
    let tga = work.join("ttk_in.tga");
    let out_name = if stp { "ttk_out.stp" } else { "ttk_out.mip" };
    let out = work.join(out_name);
    let _ = std::fs::remove_file(&out);
    write_tga(&tga, &img)?;

    let (tool, mut args) = if stp {
        // makestp [-truecolor] input output; transparency comes from the TGA itself.
        let mut a: Vec<String> = Vec::new();
        if recipe.family == Family::BestQuality {
            a.push("-truecolor".into());
        }
        ("makestp.exe", a)
    } else {
        ("makemip.exe", recipe.makemip_flags(source_alpha))
    };
    args.push("ttk_in.tga".into());
    args.push(out_name.into());
    let quiet = Logger::new(|_, _| {});
    let res = run_tool(&quiet, &sandbox_dir.join(tool), &args, work)?;
    let _ = std::fs::remove_file(&tga);
    if !res.ok() || !out.is_file() {
        bail!(
            "{} couldn't make the picture: {}",
            tool.trim_end_matches(".exe"),
            res.last_line()
        );
    }

    // Check the result reads back before touching the real file.
    let built = Mip::open(&out).context("the new texture didn't read back")?;
    built
        .decode_largest()
        .context("the new texture didn't decode")?;
    let info = built.info.clone();

    let backup = if target.is_file() {
        let dir = target
            .parent()
            .unwrap_or(Path::new("."))
            .join("_ttk_backup");
        Some(util::backup_file(target, &dir)?)
    } else {
        None
    };
    util::move_file(&out, target)?;
    Ok(Built {
        target: target.to_path_buf(),
        info,
        backup,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags() {
        let r = Recipe::new_texture(true, false);
        assert_eq!(
            r.makemip_flags(Transparency::None),
            ["-dxt", "-tile", "-priority", "0.4"]
        );
        let r = Recipe {
            family: Family::Standard,
            mapping: 2,
            priority: 0.1,
            single_level: true,
            size: None,
            cutout_type: true,
            original_kind: Some(4),
        };
        assert_eq!(
            r.makemip_flags(Transparency::Holes),
            ["-trans", "-tileU", "-priority", "0.1", "-single"]
        );
        assert_eq!(
            r.makemip_flags(Transparency::Soft),
            ["-tileU", "-priority", "0.1", "-single"]
        );
    }

    #[test]
    fn stems() {
        assert_eq!(safe_stem("My Sign (v2)"), "My_Sign__v2_");
        assert_eq!(safe_stem(""), "texture");
    }
}
