//! Step 5: copy the .dat into NR2003 and seed the support files from a template track.
//!
//! Existing files in the game folder are never overwritten silently:
//! the .dat is backed up to `_ttk_backup\` before being replaced, and template
//! files are only copied when missing.

use super::Ctx;
use crate::inifile::IniDoc;
use crate::util::{backup_file, ext_lower, file_name, human_size, same_dir};
use anyhow::{bail, Context, Result};
use std::fs;

pub fn run(ctx: &Ctx) -> Result<()> {
    let td = ctx.track_dir();
    let gd = ctx.game_track_dir();
    let dat_name = ctx.dat_name();
    let ws_dat = td.join(&dat_name);
    if !ws_dat.is_file() {
        bail!("{} not found — run Pack first", ws_dat.display());
    }
    if !ctx.settings.game_tracks().is_dir() {
        bail!(
            "NR2003 tracks folder not found: {} (check Settings)",
            ctx.settings.game_tracks().display()
        );
    }
    fs::create_dir_all(&gd).with_context(|| format!("creating {}", gd.display()))?;

    if same_dir(&td, &gd) {
        ctx.log
            .info("Workspace track folder is the game folder — .dat is already in place.");
    } else {
        let game_dat = gd.join(&dat_name);
        if game_dat.is_file() {
            let b = backup_file(&game_dat, &gd.join("_ttk_backup"))?;
            ctx.log
                .info(format!("Backed up the previous .dat to {}", b.display()));
        }
        fs::copy(&ws_dat, &game_dat)?;
        ctx.log.ok(format!(
            "Installed {} ({}) -> {}",
            dat_name,
            human_size(fs::metadata(&game_dat)?.len()),
            gd.display()
        ));
    }

    if ctx.opts.copy_templates {
        copy_templates(ctx)?;
    }

    let lps = fs::read_dir(&gd)?
        .filter_map(|e| e.ok())
        .filter(|e| ext_lower(&e.path()) == "lp")
        .count();
    if lps == 0 {
        ctx.log.warn(
            "No .lp files in the game folder — NR2003 needs a full LP set to load the track.",
        );
    }
    if !gd.join("track.ini").is_file() {
        ctx.log
            .warn("No track.ini in the game folder — NR2003 won't list the track.");
    }
    Ok(())
}

fn copy_templates(ctx: &Ctx) -> Result<()> {
    let template = ctx.settings.template_track.trim();
    if template.is_empty() {
        ctx.log
            .info("No template track set; skipping support files.");
        return Ok(());
    }
    let src_dir = ctx.settings.game_tracks().join(template);
    if !src_dir.is_dir() {
        bail!("template track not found: {}", src_dir.display());
    }
    let gd = ctx.game_track_dir();
    if same_dir(&src_dir, &gd) {
        return Ok(());
    }

    let mut copied = Vec::new();
    let mut kept = 0;
    let mut new_ini = false;
    for e in fs::read_dir(&src_dir)?.filter_map(|e| e.ok()) {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let ext = ext_lower(&p);
        if matches!(ext.as_str(), "dat" | "bak" | "log" | "def") {
            continue;
        }
        // <template>.cam -> <name>.cam; everything else keeps its name.
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dest_name = if stem.eq_ignore_ascii_case(template) {
            format!("{}.{}", ctx.name, ext)
        } else {
            file_name(&p)
        };
        let dest = gd.join(&dest_name);
        if dest.exists() {
            kept += 1;
            continue;
        }
        fs::copy(&p, &dest)?;
        if dest_name.eq_ignore_ascii_case("track.ini") {
            new_ini = true;
        }
        copied.push(dest_name);
    }

    if new_ini {
        let ini_path = gd.join("track.ini");
        let mut doc = IniDoc::parse(&fs::read(&ini_path)?);
        doc.set("track", "track_name", &ctx.name.to_uppercase());
        doc.set("track", "track_name_short", ctx.name);
        fs::write(&ini_path, doc.to_bytes())?;
        ctx.log.info(format!(
            "track.ini: set track_name to {}",
            ctx.name.to_uppercase()
        ));
    }

    if copied.is_empty() {
        ctx.log
            .info(format!("Support files already present ({kept} kept)."));
    } else {
        copied.sort_by_key(|s| s.to_ascii_lowercase());
        ctx.log.ok(format!(
            "Copied {} support files from '{template}': {}",
            copied.len(),
            copied.join(", ")
        ));
        if copied
            .iter()
            .any(|c| c.to_ascii_lowercase().ends_with(".lp"))
        {
            ctx.log.warn(format!(
                "The .lp files are placeholders from '{template}' and won't match this layout — \
                 replace them before racing."
            ));
        }
        if kept > 0 {
            ctx.log
                .info(format!("{kept} existing file(s) kept unchanged."));
        }
    }
    Ok(())
}
