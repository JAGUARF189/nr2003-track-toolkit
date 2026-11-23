//! Step 4: write `<name>.def` and build `<name>.dat` with mklib.
//!
//! mklib quirks (verified against the 2003 tool):
//! - the working form is `mklib [-c] [-xext] <path\to\file.def>`; the output
//!   name comes from the LIBNAME line, and a second positional argument makes
//!   it fail with "Couldn't open .DEF file"
//! - RAW paths are relative to the working directory

use super::Ctx;
use crate::runner::run_tool;
use crate::util::{file_name, human_size, list_files, parse_exts};
use anyhow::{bail, Result};
use std::collections::HashSet;
use std::fs;

pub fn run(ctx: &Ctx) -> Result<()> {
    let td = ctx.track_dir();
    if !td.is_dir() {
        bail!(
            "track folder doesn't exist: {} (run Workspace first)",
            td.display()
        );
    }
    let exts = parse_exts(&ctx.opts.pack_exts);
    if exts.is_empty() {
        bail!("no file types to pack (set 'File types' in the Pack section)");
    }

    let mut entries: Vec<String> = list_files(&td, &exts)?
        .iter()
        .map(|p| file_name(p))
        .collect();
    let tm = td.join("trackmat");
    if tm.is_dir() {
        entries.extend(
            list_files(&tm, &exts)?
                .iter()
                .map(|p| format!("trackmat\\{}", file_name(p))),
        );
    }
    if entries.is_empty() {
        bail!("nothing to pack in {}", td.display());
    }

    let ptf = format!("{}.ptf", ctx.name).to_ascii_lowercase();
    if !entries.iter().any(|e| e.to_ascii_lowercase() == ptf) {
        let others: Vec<&String> = entries
            .iter()
            .filter(|e| e.to_ascii_lowercase().ends_with(".ptf"))
            .collect();
        if others.is_empty() {
            ctx.log.warn(format!(
                "No {}.ptf found — save the track from Sandbox into this folder first.",
                ctx.name
            ));
        } else {
            ctx.log.warn(format!(
                "No {}.ptf, but found {}. The game looks for {}.ptf.",
                ctx.name,
                others
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                ctx.name
            ));
        }
    }
    for e in &entries {
        if e.contains(' ') {
            ctx.log.warn(format!(
                "'{e}' has a space in its name; mklib may reject it"
            ));
        }
    }

    let dat_name = ctx.dat_name();
    let mut def = format!("LIBNAME={dat_name}\r\n");
    for e in &entries {
        def.push_str("RAW=");
        def.push_str(e);
        def.push_str("\r\n");
    }
    let def_path = td.join(format!("{}.def", ctx.name));
    fs::write(&def_path, def)?;
    ctx.log.info(format!(
        "Wrote {} ({} files)",
        file_name(&def_path),
        entries.len()
    ));

    let mut args: Vec<String> = Vec::new();
    if ctx.opts.compress {
        args.push("-c".into());
        for x in parse_exts(&ctx.opts.no_compress_exts) {
            args.push(format!("-x{x}"));
        }
    }
    args.push(def_path.to_string_lossy().into_owned());
    let out = run_tool(ctx.log, &ctx.tool("mklib.exe"), &args, &td)?;
    let dat = td.join(&dat_name);
    if !out.ok() || !dat.is_file() {
        bail!("mklib failed (exit {}): {}", out.code, out.last_line());
    }

    // Verify by listing the new archive.
    let list = run_tool_quiet(ctx, &["-l", dat_name.as_str()])?;
    let listed = parse_listing(&list);
    let have: HashSet<String> = listed.iter().map(|s| s.to_ascii_lowercase()).collect();
    let missing: Vec<&String> = entries
        .iter()
        .filter(|e| !have.contains(&e.to_ascii_lowercase()))
        .collect();
    if !missing.is_empty() || listed.len() != entries.len() {
        bail!(
            "{dat_name} lists {} entries but {} were packed{}",
            listed.len(),
            entries.len(),
            if missing.is_empty() {
                String::new()
            } else {
                format!(
                    "; missing: {}",
                    missing
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        );
    }
    let size = fs::metadata(&dat)?.len();
    ctx.log.ok(format!(
        "Built {dat_name}: {} files, {} — listing verified",
        listed.len(),
        human_size(size)
    ));
    Ok(())
}

/// Entry names from `mklib -l` output:
/// a header, a dashed rule, then `<name> <size>` lines, a blank line and `Total Size: N bytes`.
pub fn parse_listing(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .skip_while(|l| !l.trim_start().starts_with("---"))
        .skip(1)
        .filter_map(|l| {
            let t = l.trim();
            let (name, size) = t.rsplit_once(char::is_whitespace)?;
            if t.starts_with("Total") || !size.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some(name.trim().to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn listing() {
        let out: Vec<String> = [
            "",
            "NAME                                 SIZE",
            "--------------------------------------------",
            "                       trkinfo.stp   24272",
            "              trackmat\\wallurl.mip   53152",
            "",
            "Total Size: 9978606 bytes",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            super::parse_listing(&out),
            vec!["trkinfo.stp", "trackmat\\wallurl.mip"]
        );
    }
}

/// Run mklib without echoing every line to the log.
fn run_tool_quiet(ctx: &Ctx, args: &[&str]) -> Result<Vec<String>> {
    let silent = crate::logger::Logger::new(|_, _| {});
    let out = run_tool(&silent, &ctx.tool("mklib.exe"), args, &ctx.track_dir())?;
    if !out.ok() {
        bail!(
            "mklib {} failed (exit {}): {}",
            args.join(" "),
            out.code,
            out.last_line()
        );
    }
    Ok(out.lines)
}
