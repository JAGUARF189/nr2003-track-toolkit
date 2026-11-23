//! Step 1: the folder layout Sandbox insists on.
//!
//! tracks\shared            unpacked shared.dat
//! tracks\<name>            TSOs (.3do), TSDs and TSO textures (.mip), .ptf
//! tracks\<name>\trackmat   track surface textures (.mip)

use super::{validate_track_name, Ctx};
use crate::config::TrackOpts;
use crate::runner::run_tool;
use crate::util::{file_name, resolve, same_dir};
use anyhow::{bail, Context, Result};
use std::fs;

const MARKER: &str = ".ttk_unpacked";

/// Textures Sandbox needs in the track folder before flaggers can be placed.
const FLAGGER_MIPS: [&str; 9] = [
    "flag_black.mip",
    "flag_blue.mip",
    "flag_checkered.mip",
    "flag_green.mip",
    "flag_red.mip",
    "flag_white.mip",
    "flag_yellow.mip",
    "series_flagger.mip",
    "stopgo.mip",
];

pub fn run(ctx: &Ctx) -> Result<()> {
    validate_track_name(ctx.name)?;
    let ws = &ctx.settings.workspace_dir;
    let td = ctx.track_dir();
    let is_new = !td.exists();

    fs::create_dir_all(td.join("trackmat"))
        .with_context(|| format!("creating {}", td.display()))?;
    if is_new {
        ctx.log.ok(format!("Created {}", td.display()));
    } else {
        ctx.log.info(format!("Track folder: {}", td.display()));
    }
    for job in &ctx.opts.tex_jobs {
        let src = resolve(&td, &job.src);
        if !src.exists() {
            fs::create_dir_all(&src)?;
            ctx.log
                .info(format!("Created texture source folder {}", src.display()));
        }
    }
    if !td.join(TrackOpts::FILE).exists() {
        ctx.opts.save(&td)?;
    }

    unpack_shared(ctx)?;
    ctx.check_cancel()?;
    import_dat(ctx)?;
    ctx.check_cancel()?;

    if ctx.opts.copy_flagger_mips {
        let shared = ctx.settings.shared_dir();
        let mut copied = 0;
        let mut missing = Vec::new();
        for f in FLAGGER_MIPS {
            let dst = td.join(f);
            if dst.exists() {
                continue;
            }
            let src = shared.join(f);
            if src.is_file() {
                fs::copy(&src, &dst)?;
                copied += 1;
            } else {
                missing.push(f);
            }
        }
        if copied > 0 {
            ctx.log.ok(format!(
                "Copied {copied} flagger textures into the track folder"
            ));
        }
        if !missing.is_empty() {
            ctx.log.warn(format!(
                "Not found in {}: {}",
                shared.display(),
                missing.join(", ")
            ));
        }
    }

    if !ws.join(ctx.name).join(format!("{}.ptf", ctx.name)).exists() {
        ctx.log.info(format!(
            "Next: open Sandbox, place a segment and save it as {}\\{}.ptf, then File > New and reopen it \
             (Sandbox only shows textures after that reload).",
            td.display(),
            ctx.name
        ));
    }
    Ok(())
}

fn unpack_shared(ctx: &Ctx) -> Result<()> {
    let ws = &ctx.settings.workspace_dir;
    let game_tracks = ctx.settings.game_tracks();
    let shared = ctx.settings.shared_dir();

    if same_dir(ws, &game_tracks) {
        ctx.log.warn(
            "The workspace is the game's own tracks folder, so tracks\\shared is left alone \
             (shared.dat won't be unpacked into the game install).",
        );
        return Ok(());
    }
    if shared.join(MARKER).exists() {
        ctx.log.info(format!(
            "Unpacked shared already present: {}",
            shared.display()
        ));
        return Ok(());
    }

    let src_dat = game_tracks.join("shared").join("shared.dat");
    if !src_dat.is_file() {
        bail!(
            "shared.dat not found at {} (check the NR2003 folder in Settings)",
            src_dat.display()
        );
    }
    fs::create_dir_all(&shared)?;
    let dst_dat = shared.join("shared.dat");
    let needs_copy = match (fs::metadata(&src_dat), fs::metadata(&dst_dat)) {
        (Ok(a), Ok(b)) => a.len() != b.len(),
        _ => true,
    };
    if needs_copy {
        ctx.log.info(format!("Copying {} …", src_dat.display()));
        fs::copy(&src_dat, &dst_dat)?;
    }
    ctx.log.info("Unpacking shared.dat (this takes a moment) …");
    let out = run_tool(
        ctx.log,
        &ctx.tool("mklib.exe"),
        &["-u", "shared.dat"],
        &shared,
    )?;
    if !out.ok() {
        bail!(
            "mklib -u shared.dat failed (exit {}): {}",
            out.code,
            out.last_line()
        );
    }
    let n = out
        .lines
        .iter()
        .filter(|l| l.trim_start().starts_with("unpacking"))
        .count();
    fs::write(
        shared.join(MARKER),
        format!("unpacked {n} files from shared.dat\n"),
    )?;
    ctx.log
        .ok(format!("Unpacked {n} files into {}", shared.display()));
    Ok(())
}

/// Optionally seed the track folder from an existing track's .dat.
fn import_dat(ctx: &Ctx) -> Result<()> {
    let spec = ctx.opts.import_dat.trim();
    if spec.is_empty() {
        return Ok(());
    }
    let td = ctx.track_dir();
    let src = resolve(&ctx.settings.game_tracks(), spec);
    if !src.is_file() {
        bail!("import .dat not found: {}", src.display());
    }
    let has_work = fs::read_dir(&td)?
        .filter_map(|e| e.ok())
        .any(|e| matches!(crate::util::ext_lower(&e.path()).as_str(), "ptf" | "3do"));
    if has_work {
        ctx.log.info(format!(
            "Skipping import of {}: the track folder already has .ptf/.3do files (clear 'Import .dat' to hide this).",
            file_name(&src)
        ));
        return Ok(());
    }
    // mklib -u unpacks into the current directory, so unpack a copy in place.
    let tmp_name = "_ttk_import.dat";
    let tmp = td.join(tmp_name);
    fs::copy(&src, &tmp)?;
    ctx.log.info(format!(
        "Unpacking {} into the track folder …",
        src.display()
    ));
    let out = run_tool(ctx.log, &ctx.tool("mklib.exe"), &["-u", tmp_name], &td);
    let _ = fs::remove_file(&tmp);
    let out = out?;
    if !out.ok() {
        bail!("mklib -u failed (exit {}): {}", out.code, out.last_line());
    }
    let n = out
        .lines
        .iter()
        .filter(|l| l.trim_start().starts_with("unpacking"))
        .count();
    ctx.log
        .ok(format!("Imported {n} files from {}", file_name(&src)));

    // A stock track's .ptf is named after that track; Sandbox and the packer
    // expect <name>.ptf.
    let src_stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let from = td.join(format!("{src_stem}.ptf"));
    let to = td.join(format!("{}.ptf", ctx.name));
    if !src_stem.eq_ignore_ascii_case(ctx.name) && from.is_file() && !to.exists() {
        fs::rename(&from, &to)?;
        ctx.log
            .info(format!("Renamed {src_stem}.ptf -> {}.ptf", ctx.name));
    }
    Ok(())
}
