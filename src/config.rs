use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Machine-wide settings, stored in `%APPDATA%\TrackToolkit\settings.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Folder holding mklib.exe, makemip.exe, make3do.exe, mkstalls.exe.
    pub sandbox_dir: PathBuf,
    /// NR2003 install (the folder that contains `tracks\`).
    pub nr2003_dir: PathBuf,
    /// The Level-1 `tracks` folder you build in (Sandbox's workspace).
    pub workspace_dir: PathBuf,
    /// Stock track whose ini/cam/sim/lp files seed new tracks.
    pub template_track: String,
    pub last_track: String,
    /// Track tab: save changes automatically.
    pub autosave: bool,
    /// Track tab: seconds between automatic saves (only when there are unsaved changes).
    pub autosave_secs: u64,
    /// Assetto Corsa's `content\tracks` folder (empty: look for it).
    pub ac_tracks_dir: PathBuf,
    /// Installed NR2003 track whose ground textures, walls and settings AC conversions copy.
    pub ac_template: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sandbox_dir: PathBuf::from(r"C:\Program Files (x86)\sandbox"),
            nr2003_dir: PathBuf::from(r"C:\Papyrus\NASCAR Racing 2003 Season"),
            workspace_dir: PathBuf::from(r"C:\Papyrus\TrackWorkspace\tracks"),
            template_track: "hillside".into(),
            last_track: String::new(),
            autosave: true,
            autosave_secs: 300,
            ac_tracks_dir: PathBuf::new(),
            ac_template: "watkins_glen".into(),
        }
    }
}

impl Settings {
    pub fn default_path() -> PathBuf {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("TrackToolkit")
            .join("settings.json")
    }

    pub fn load_from(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn load() -> Self {
        Self::load_from(&Self::default_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::default_path())
    }

    /// Where autosave keeps safety copies of tracks that don't have a save location yet.
    pub fn recovery_dir() -> PathBuf {
        Self::default_path().parent().map(Path::to_path_buf).unwrap_or_default().join("autosave")
    }

    pub fn game_tracks(&self) -> PathBuf {
        self.nr2003_dir.join("tracks")
    }

    pub fn track_dir(&self, name: &str) -> PathBuf {
        self.workspace_dir.join(name)
    }

    pub fn shared_dir(&self) -> PathBuf {
        self.workspace_dir.join("shared")
    }

    /// Track folders in the workspace (everything except `shared`).
    pub fn list_tracks(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(&self.workspace_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| !n.eq_ignore_ascii_case("shared"))
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by_key(|s| s.to_ascii_lowercase());
        v
    }

    /// Track folders in the NR2003 install that have a `<name>.dat`.
    pub fn list_game_tracks(&self) -> Vec<String> {
        let gt = self.game_tracks();
        let mut v: Vec<String> = fs::read_dir(&gt)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| gt.join(n).join(format!("{n}.dat")).is_file())
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by_key(|s| s.to_ascii_lowercase());
        v
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TexDest {
    /// `tracks\<name>\trackmat` — track surface textures
    Trackmat,
    /// `tracks\<name>` — TSO / TSD textures
    TrackRoot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resize {
    /// Fail on non-power-of-two images
    Off,
    /// Closest power of two per side
    Nearest,
    /// Next power of two up
    Up,
    /// Next power of two down
    Down,
}

impl Resize {
    pub const ALL: [Resize; 4] = [Resize::Nearest, Resize::Up, Resize::Down, Resize::Off];

    pub fn label(self) -> &'static str {
        match self {
            Resize::Off => "Off (fail if not power of 2)",
            Resize::Nearest => "Nearest power of 2",
            Resize::Up => "Up to power of 2",
            Resize::Down => "Down to power of 2",
        }
    }

    pub fn apply(self, n: u32) -> Option<u32> {
        if n.is_power_of_two() {
            return Some(n);
        }
        let up = n.next_power_of_two();
        let down = up / 2;
        match self {
            Resize::Off => None,
            Resize::Up => Some(up),
            Resize::Down => Some(down.max(1)),
            Resize::Nearest => Some(if up - n <= n - down { up } else { down.max(1) }),
        }
    }
}

/// One folder of source images and how to turn them into MIPs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TexJob {
    pub label: String,
    /// Source folder (relative to the track folder, or absolute).
    pub src: String,
    pub dest: TexDest,
    pub resize: Resize,
    /// makemip -tile
    pub tile: bool,
    /// makemip -dxt
    pub dxt: bool,
    /// makemip -trueColor
    pub true_color: bool,
    /// makemip -trans
    pub trans: bool,
    /// Rebuild even if the .mip is newer than the source
    pub force: bool,
}

impl Default for TexJob {
    fn default() -> Self {
        Self {
            label: "Textures".into(),
            src: String::new(),
            dest: TexDest::TrackRoot,
            resize: Resize::Nearest,
            tile: false,
            dxt: false,
            true_color: false,
            trans: false,
            force: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StallOpts {
    /// Include in "Run all"
    pub enabled: bool,
    /// LP file in the installed track folder, e.g. pit0.lp
    pub lp_file: String,
    /// track.ini section to write into, e.g. pit_lane_0 or starting_grid_0
    pub section: String,
    pub start_dlong: f64,
    pub end_dlong: f64,
    pub count: u32,
    /// 0 = single pit lane; negative = pole on right; positive = pole on left
    pub grid_offset: f64,
}

impl Default for StallOpts {
    fn default() -> Self {
        Self {
            enabled: false,
            lp_file: "pit0.lp".into(),
            section: "pit_lane_0".into(),
            start_dlong: 0.0,
            end_dlong: 0.0,
            count: 43,
            grid_offset: 0.0,
        }
    }
}

/// Per-track options, stored in `tracks\<name>\toolkit.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackOpts {
    /// Optional .dat to unpack into a fresh track folder (start from an existing track).
    pub import_dat: String,
    pub copy_flagger_mips: bool,
    pub tex_jobs: Vec<TexJob>,
    /// Folder with .psg scripts (relative to the track folder; empty = track folder).
    pub psg_dir: String,
    pub make3do_verbose: bool,
    /// File types packed into the .dat (root + trackmat\).
    pub pack_exts: String,
    pub compress: bool,
    pub no_compress_exts: String,
    pub copy_templates: bool,
    pub stalls: StallOpts,
}

impl Default for TrackOpts {
    fn default() -> Self {
        Self {
            import_dat: String::new(),
            copy_flagger_mips: true,
            tex_jobs: vec![
                TexJob {
                    label: "Track surfaces".into(),
                    src: r"src_textures\trackmat".into(),
                    dest: TexDest::Trackmat,
                    tile: true,
                    ..TexJob::default()
                },
                TexJob {
                    label: "Objects (TSO / TSD)".into(),
                    src: r"src_textures\objects".into(),
                    dest: TexDest::TrackRoot,
                    ..TexJob::default()
                },
            ],
            psg_dir: String::new(),
            make3do_verbose: false,
            pack_exts: "3do mip ptf pth stp".into(),
            compress: true,
            no_compress_exts: String::new(),
            copy_templates: true,
            stalls: StallOpts::default(),
        }
    }
}

impl TrackOpts {
    pub const FILE: &'static str = "toolkit.json";

    pub fn load(track_dir: &Path) -> Self {
        fs::read_to_string(track_dir.join(Self::FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, track_dir: &Path) -> Result<()> {
        let p = track_dir.join(Self::FILE);
        fs::write(&p, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", p.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::Resize;

    #[test]
    fn pow2() {
        assert_eq!(Resize::Nearest.apply(512), Some(512));
        assert_eq!(Resize::Nearest.apply(600), Some(512));
        assert_eq!(Resize::Nearest.apply(800), Some(1024));
        assert_eq!(Resize::Nearest.apply(768), Some(1024)); // tie goes up
        assert_eq!(Resize::Down.apply(1920), Some(1024));
        assert_eq!(Resize::Up.apply(1200), Some(2048));
        assert_eq!(Resize::Off.apply(1200), None);
    }
}
