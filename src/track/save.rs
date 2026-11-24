//! Saving a track without ever leaving a broken file behind.
//!
//! 1. Write the new version next to the track as `<name>.ptf.saving` (via ptfjson).
//! 2. Read that file back and check it holds exactly what was meant to be saved.
//! 3. Copy the current track file into `_ttk_backup\` (the newest [`TRACK_BACKUPS_KEPT`] are kept).
//! 4. Swap the new file into place in one rename.
//!
//! If anything fails before step 4, the track file on disk is untouched.

use super::graph::{read_ptf_file, write_ptf_json, Graph};
use crate::util;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Backups kept per track file. Autosave every 5 minutes keeps about 1h40 of history.
pub const TRACK_BACKUPS_KEPT: usize = 20;

pub const BACKUP_DIR: &str = "_ttk_backup";

#[derive(Debug)]
pub struct Saved {
    pub path: PathBuf,
    /// Copy of the previous version, if there was one.
    pub backup: Option<PathBuf>,
}

/// Save ptfjson text (from [`Graph::to_json`]) as the track file `dest`.
pub fn save_json(json: &str, dest: &Path) -> Result<Saved> {
    let dir = dest.parent().context("the save location has no folder")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = dest.with_extension("ptf.saving");
    let _ = std::fs::remove_file(&tmp);

    let written = (|| -> Result<()> {
        write_ptf_json(json, &tmp)?;
        let back = read_ptf_file(&tmp).context("reading the new file back")?;
        // Compare in the writer's numbering, so added and removed objects still match.
        let want = Graph::from_json(json)?.canonical();
        if let Some(d) = want.difference(&back) {
            bail!("the new file didn't match the track ({d})");
        }
        Ok(())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.context("saving stopped; your track file was not changed"));
    }

    let backup = if dest.is_file() {
        Some(
            util::backup_file_keep(dest, &dir.join(BACKUP_DIR), TRACK_BACKUPS_KEPT)
                .with_context(|| format!("backing up {}", dest.display()))?,
        )
    } else {
        None
    };
    // std::fs::rename replaces the destination in one step on Windows (MoveFileEx).
    if let Err(e) = std::fs::rename(&tmp, dest) {
        util::move_file(&tmp, dest).with_context(|| format!("replacing {} ({e})", dest.display()))?;
    }
    Ok(Saved { path: dest.to_path_buf(), backup })
}

/// Save a graph as the track file `dest`.
pub fn save_graph(g: &Graph, dest: &Path) -> Result<Saved> {
    save_json(&g.to_json()?, dest)
}

/// Earlier versions of a track file, newest first: (backup file, when it was made).
pub fn backups(dest: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Some(dir) = dest.parent().map(|d| d.join(BACKUP_DIR)) else { return Vec::new() };
    let prefix = format!("{}.", util::file_name(dest).to_ascii_lowercase());
    let mut out: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| util::file_name(p).to_ascii_lowercase().starts_with(&prefix))
                .filter_map(|p| util::mtime(&p).map(|t| (p, t)))
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| util::file_name(&b.0).cmp(&util::file_name(&a.0)));
    out
}
