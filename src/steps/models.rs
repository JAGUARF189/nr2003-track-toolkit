//! Step 3: run every .psg script through `make3do -s`.

use super::Ctx;
use crate::runner::run_tool;
use crate::util::{file_name, list_files, mtime, resolve};
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

fn snapshot_3do(dir: &Path) -> HashMap<PathBuf, Option<SystemTime>> {
    list_files(dir, &["3do".to_string()])
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let t = mtime(&p);
            (p, t)
        })
        .collect()
}

pub fn run(ctx: &Ctx) -> Result<()> {
    let td = ctx.track_dir();
    let dir = resolve(&td, &ctx.opts.psg_dir);
    if !dir.is_dir() {
        bail!("PSG folder doesn't exist: {}", dir.display());
    }
    let scripts = list_files(&dir, &["psg".to_string()])?;
    if scripts.is_empty() {
        ctx.log.info(format!(
            "No .psg scripts in {} — nothing to build.",
            dir.display()
        ));
        return Ok(());
    }

    let before = snapshot_3do(&dir);
    let mut failed = Vec::new();
    for s in &scripts {
        ctx.check_cancel()?;
        let name = file_name(s);
        let mut args: Vec<String> = Vec::new();
        if ctx.opts.make3do_verbose {
            args.push("-v".into());
        }
        args.push("-s".into());
        args.push(name.clone());
        let out = run_tool(ctx.log, &ctx.tool("make3do.exe"), &args, &dir)?;
        let errs = out.error_lines();
        if !out.ok() || !errs.is_empty() {
            ctx.log.error(format!(
                "  {name}: failed (exit {}){}",
                out.code,
                errs.first()
                    .map(|e| format!(": {}", e.trim()))
                    .unwrap_or_default()
            ));
            failed.push(name);
        }
    }

    let after = snapshot_3do(&dir);
    let mut changed: Vec<String> = after
        .iter()
        .filter(|(p, t)| before.get(*p) != Some(*t))
        .map(|(p, _)| file_name(p))
        .collect();
    changed.sort_by_key(|s| s.to_ascii_lowercase());
    if !changed.is_empty() {
        ctx.log.ok(format!(
            "Wrote {} .3do: {}",
            changed.len(),
            changed.join(", ")
        ));
    }
    if !dir.eq(&td) && !changed.is_empty() {
        ctx.log.warn(format!(
            "The .3do files were written to {}; the packer only picks up files in the track folder.",
            dir.display()
        ));
    }
    if !failed.is_empty() {
        bail!(
            "{} of {} scripts failed: {}",
            failed.len(),
            scripts.len(),
            failed.join(", ")
        );
    }
    ctx.log
        .ok(format!("{} script(s) ran cleanly", scripts.len()));
    Ok(())
}
