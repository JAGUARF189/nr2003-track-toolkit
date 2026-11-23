//! Step 6: generate pit stalls / a starting grid with mkstalls and merge them
//! into the installed track.ini.

use super::Ctx;
use crate::inifile::{key_of_line, IniDoc};
use crate::runner::run_tool;
use crate::util::backup_file;
use anyhow::{bail, Result};
use std::fs;

pub fn run(ctx: &Ctx) -> Result<()> {
    let o = &ctx.opts.stalls;
    let gd = ctx.game_track_dir();
    let lp = gd.join(o.lp_file.trim());
    let ini = gd.join("track.ini");
    if o.lp_file.trim().is_empty() || !lp.is_file() {
        bail!(
            "LP file not found: {} (run Install first, or set the LP file)",
            lp.display()
        );
    }
    if !ini.is_file() {
        bail!("{} not found — run Install first", ini.display());
    }
    if o.count == 0 {
        bail!("number of stalls must be at least 1");
    }
    if o.start_dlong == o.end_dlong {
        bail!(
            "start and end dlong are the same ({}) — set the stall range",
            o.start_dlong
        );
    }
    let section = o.section.trim();
    if section.is_empty() {
        bail!("no track.ini section set (e.g. pit_lane_0 or starting_grid_0)");
    }

    let build = ctx.build_dir("stalls")?;
    let lp_name = lp.file_name().unwrap().to_string_lossy().into_owned();
    fs::copy(&lp, build.join(&lp_name))?;
    let out_ini = build.join("stalls.ini");
    let _ = fs::remove_file(&out_ini);

    let mut args = vec![
        lp_name,
        format!("{}", o.start_dlong),
        format!("{}", o.end_dlong),
        format!("{}", o.count),
    ];
    if o.grid_offset != 0.0 {
        args.push(format!("{}", o.grid_offset));
    }
    let out = run_tool(ctx.log, &ctx.tool("mkstalls.exe"), &args, &build)?;
    if !out.ok() || !out_ini.is_file() {
        bail!("mkstalls failed (exit {}): {}", out.code, out.last_line());
    }

    let generated: Vec<String> = String::from_utf8_lossy(&fs::read(&out_ini)?)
        .lines()
        .filter(|l| {
            key_of_line(l.as_bytes())
                .map(|k| k.starts_with("stall_") && k != "stall_pace")
                .unwrap_or(false)
        })
        .map(|l| l.trim().to_string())
        .collect();
    if generated.is_empty() {
        bail!("mkstalls produced no stall lines");
    }

    let b = backup_file(&ini, &gd.join("_ttk_backup"))?;
    let mut doc = IniDoc::parse(&fs::read(&ini)?);
    let existed = doc.section_range(section).is_some();
    let removed = doc.replace_stalls(section, &generated);
    fs::write(&ini, doc.to_bytes())?;
    ctx.log.ok(format!(
        "[ {section} ]: wrote {} stalls (replaced {removed}){}",
        generated.len(),
        if existed {
            ""
        } else {
            " — new section added"
        }
    ));
    ctx.log.info(format!("track.ini backup: {}", b.display()));
    if doc.get(section, "stall_pace").is_none() {
        ctx.log.warn(format!(
            "[ {section} ] has no stall_pace line; add one for the pace car."
        ));
    }
    Ok(())
}
