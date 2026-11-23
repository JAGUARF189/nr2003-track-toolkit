//! Step 2: source images -> TGA -> MIP.

use super::Ctx;
use crate::config::{TexDest, TexJob};
use crate::runner::run_tool;
use crate::tga::write_tga;
use crate::util::{ext_lower, file_name, is_up_to_date, list_files, move_file, resolve};
use anyhow::{bail, Context, Result};
use image::imageops::FilterType;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

const IMAGE_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "bmp", "tga"];

enum Outcome {
    Built,
    UpToDate,
}

pub fn run(ctx: &Ctx) -> Result<()> {
    if ctx.opts.tex_jobs.is_empty() {
        ctx.log.info("No texture folders configured.");
        return Ok(());
    }
    let mut failed = 0usize;
    for job in &ctx.opts.tex_jobs {
        failed += run_job(ctx, job)?;
    }
    if failed > 0 {
        bail!("{failed} texture(s) failed — see the messages above");
    }
    Ok(())
}

/// Returns the number of files that failed.
fn run_job(ctx: &Ctx, job: &TexJob) -> Result<usize> {
    let td = ctx.track_dir();
    let src = resolve(&td, &job.src);
    let dest = match job.dest {
        TexDest::Trackmat => td.join("trackmat"),
        TexDest::TrackRoot => td.clone(),
    };
    ctx.log.info(format!(
        "{}: {} -> {}",
        job.label,
        src.display(),
        dest.display()
    ));
    if !src.is_dir() {
        ctx.log
            .warn(format!("  source folder doesn't exist: {}", src.display()));
        return Ok(0);
    }
    fs::create_dir_all(&dest)?;
    let exts: Vec<String> = IMAGE_EXTS.iter().map(|s| s.to_string()).collect();
    let files = list_files(&src, &exts)?;
    if files.is_empty() {
        ctx.log.info("  no images found");
        return Ok(0);
    }

    let build = ctx.build_dir("tga")?;
    let mut seen: HashMap<String, String> = HashMap::new();
    let (mut built, mut current, mut failed) = (0, 0, 0);
    for f in &files {
        ctx.check_cancel()?;
        let stem = f.file_stem().unwrap().to_string_lossy().into_owned();
        if let Some(prev) = seen.insert(stem.to_ascii_lowercase(), file_name(f)) {
            ctx.log.error(format!(
                "  {}: same name as {prev} — both would become {stem}.mip; skipped",
                file_name(f)
            ));
            failed += 1;
            continue;
        }
        match convert_one(ctx, job, f, &stem, &dest, &build) {
            Ok(Outcome::Built) => built += 1,
            Ok(Outcome::UpToDate) => current += 1,
            Err(e) => {
                ctx.log.error(format!("  {}: {e:#}", file_name(f)));
                failed += 1;
            }
        }
    }
    let msg = format!("  {built} built, {current} up to date, {failed} failed");
    if failed > 0 {
        ctx.log.warn(msg);
    } else {
        ctx.log.ok(msg);
    }
    Ok(failed)
}

fn convert_one(
    ctx: &Ctx,
    job: &TexJob,
    src: &Path,
    stem: &str,
    dest: &Path,
    build: &Path,
) -> Result<Outcome> {
    let out = dest.join(format!("{stem}.mip"));
    if !job.force && is_up_to_date(&out, src) {
        return Ok(Outcome::UpToDate);
    }
    if stem.contains(' ') {
        ctx.log.warn(format!(
            "  {}: spaces in texture names can break .pas/.psg references",
            file_name(src)
        ));
    }

    let img = image::open(src)
        .with_context(|| format!("can't read {} image", ext_lower(src)))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    let (tw, th) = match (job.resize.apply(w), job.resize.apply(h)) {
        (Some(a), Some(b)) => (a, b),
        _ => bail!("{w}x{h} is not a power of two (makemip requires it) — turn on resizing or fix the image"),
    };
    let img = if (tw, th) != (w, h) {
        ctx.log.info(format!(
            "  {}: resized {w}x{h} -> {tw}x{th}",
            file_name(src)
        ));
        image::imageops::resize(&img, tw, th, FilterType::Lanczos3)
    } else {
        img
    };

    // makemip gets short relative names in a scratch folder: no spaces, no long paths.
    let tga_name = format!("{stem}.tga");
    let mip_name = format!("{stem}.mip");
    let tga = build.join(&tga_name);
    let tmp_mip = build.join(&mip_name);
    let _ = fs::remove_file(&tmp_mip);
    let alpha = write_tga(&tga, &img)?;

    let mut args: Vec<String> = Vec::new();
    if job.tile {
        args.push("-tile".into());
    }
    if job.dxt {
        args.push("-dxt".into());
    }
    if job.true_color {
        args.push("-trueColor".into());
    }
    if job.trans {
        args.push("-trans".into());
    }
    args.push(tga_name);
    args.push(mip_name);

    let res = run_tool(ctx.log, &ctx.tool("makemip.exe"), &args, build)?;
    let _ = fs::remove_file(&tga);
    if !res.ok() || !tmp_mip.is_file() {
        bail!("makemip failed (exit {}): {}", res.code, res.last_line());
    }
    move_file(&tmp_mip, &out)?;
    ctx.log.info(format!(
        "  {} -> {} ({tw}x{th}{})",
        file_name(src),
        file_name(&out),
        if alpha { ", alpha" } else { "" }
    ));
    Ok(Outcome::Built)
}
