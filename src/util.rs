use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Resolve `p` against `base` unless it is absolute. Empty means `base`.
pub fn resolve(base: &Path, p: &str) -> PathBuf {
    let p = p.trim();
    if p.is_empty() || p == "." {
        return base.to_path_buf();
    }
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        base.join(pb)
    }
}

pub fn ext_lower(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn mtime(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// True when `out` exists and is at least as new as `src`.
pub fn is_up_to_date(out: &Path, src: &Path) -> bool {
    match (mtime(out), mtime(src)) {
        (Some(o), Some(s)) => o >= s,
        _ => false,
    }
}

pub fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// UTC timestamp like `20261122-183800Z`, safe for file names.
pub fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}Z",
        y,
        m,
        d,
        sod / 3600,
        sod % 3600 / 60,
        sod % 60
    )
}

/// How many backups of each file `backup_file` keeps.
pub const BACKUPS_KEPT: usize = 5;

/// Copy `file` into `backup_dir` as `<name>.<timestamp>`, keeping the newest
/// [`BACKUPS_KEPT`] backups of that file.
pub fn backup_file(file: &Path, backup_dir: &Path) -> io::Result<PathBuf> {
    backup_file_keep(file, backup_dir, BACKUPS_KEPT)
}

/// [`backup_file`] keeping the newest `keep` backups.
pub fn backup_file_keep(file: &Path, backup_dir: &Path, keep: usize) -> io::Result<PathBuf> {
    fs::create_dir_all(backup_dir)?;
    let base = file_name(file);
    let dest = backup_dir.join(format!("{}.{}", base, timestamp()));
    fs::copy(file, &dest)?;

    // Timestamps sort lexically, so the oldest come first.
    let prefix = format!("{}.", base.to_ascii_lowercase());
    let mut old: Vec<PathBuf> = fs::read_dir(backup_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let n = file_name(p).to_ascii_lowercase();
            n.strip_prefix(&prefix)
                .map(|ts| ts.len() == 16 && ts.ends_with('z'))
                .unwrap_or(false)
        })
        .collect();
    old.sort_by_key(|p| file_name(p));
    if old.len() > keep {
        for p in &old[..old.len() - keep] {
            let _ = fs::remove_file(p);
        }
    }
    Ok(dest)
}

/// Move a file, replacing the destination. Falls back to copy+delete.
pub fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if to.exists() {
        fs::remove_file(to)?;
    }
    if fs::rename(from, to).is_err() {
        fs::copy(from, to)?;
        fs::remove_file(from)?;
    }
    Ok(())
}

/// Files (not dirs) directly inside `dir` whose lowercase extension is in
/// `exts` (all files if `exts` is empty), sorted case-insensitively.
pub fn list_files(dir: &Path, exts: &[String]) -> io::Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| exts.is_empty() || exts.contains(&ext_lower(p)))
        .collect();
    v.sort_by_key(|p| file_name(p).to_ascii_lowercase());
    Ok(v)
}

/// Parse "3do mip, ptf" style extension lists.
pub fn parse_exts(s: &str) -> Vec<String> {
    s.split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .map(|e| {
            e.trim()
                .trim_start_matches("*.")
                .trim_start_matches('.')
                .to_ascii_lowercase()
        })
        .filter(|e| !e.is_empty())
        .collect()
}

pub fn human_size(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f >= K * K {
        format!("{:.1} MB", f / K / K)
    } else if f >= K {
        format!("{:.1} KB", f / K)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exts() {
        assert_eq!(
            parse_exts("3do, *.MIP .ptf;stp"),
            vec!["3do", "mip", "ptf", "stp"]
        );
    }

    #[test]
    fn ts_shape() {
        let t = timestamp();
        assert_eq!(t.len(), 16);
        assert!(t.ends_with('Z'));
    }
}
