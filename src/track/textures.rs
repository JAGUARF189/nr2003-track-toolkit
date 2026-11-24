//! Finding and decoding the .mip textures a track uses, for the 3D view.
//!
//! A track names its textures like `trackmat\asphalt.mip` or `wall.mip`. They're looked for:
//! 1. at that path under the track folder,
//! 2. by file name in the track folder and its `trackmat` folder,
//! 3. inside the track folder's .dat files,
//! 4. in the game's `tracks\shared` folder.

use crate::dat::Dat;
use crate::mip::Mip;
use image::RgbaImage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

enum Source {
    File(PathBuf),
    /// (index into `dats`, entry index)
    Dat(usize, usize),
}

pub struct TexStore {
    dir: PathBuf,
    by_name: HashMap<String, Source>,
    dats: Vec<Dat>,
}

impl TexStore {
    /// Index the textures available to the track in `track_dir`.
    pub fn new(track_dir: &Path) -> Self {
        let mut by_name: HashMap<String, Source> = HashMap::new();
        let mut dats = Vec::new();
        let add_dir = |dir: &Path, by_name: &mut HashMap<String, Source>| {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = e.path();
                if crate::util::ext_lower(&p) == "mip" {
                    by_name.entry(crate::util::file_name(&p).to_ascii_lowercase()).or_insert(Source::File(p));
                }
            }
        };
        add_dir(track_dir, &mut by_name);
        add_dir(&track_dir.join("trackmat"), &mut by_name);
        for e in std::fs::read_dir(track_dir).into_iter().flatten().flatten() {
            let p = e.path();
            if crate::util::ext_lower(&p) != "dat" {
                continue;
            }
            if let Ok(d) = Dat::open(&p) {
                let k = dats.len();
                for (i, en) in d.entries.iter().enumerate() {
                    if en.ext_lower() == "mip" {
                        by_name.entry(en.file_name().to_ascii_lowercase()).or_insert(Source::Dat(k, i));
                    }
                }
                dats.push(d);
            }
        }
        if let Some(shared) = track_dir.parent().map(|t| t.join("shared")).filter(|d| d.is_dir()) {
            add_dir(&shared, &mut by_name);
        }
        Self { dir: track_dir.to_path_buf(), by_name, dats }
    }

    /// The file's bytes (from the folder, trackmat, a .dat or shared).
    pub fn bytes(&self, file: &str) -> Option<Vec<u8>> {
        let direct = self.dir.join(file.replace('\\', "/"));
        if direct.is_file() {
            return std::fs::read(direct).ok();
        }
        let base = file.rsplit(['\\', '/']).next().unwrap_or(file).to_ascii_lowercase();
        match self.by_name.get(&base)? {
            Source::File(p) => std::fs::read(p).ok(),
            Source::Dat(d, i) => {
                let dat = &self.dats[*d];
                dat.read(&dat.entries[*i]).ok()
            }
        }
    }

    /// Decode texture `file` at about `side` pixels on its longer side (the nearest stored
    /// size at least that big, else the largest). None if it can't be found or read.
    pub fn load(&self, file: &str, side: u32) -> Option<RgbaImage> {
        let mip = Mip::parse(self.bytes(file)?).ok()?;
        let level = mip.level_for(side);
        let img = mip.decode_level(level).ok()?;
        // Big single-level textures: shrink so the 3D view doesn't hold hundreds of megabytes.
        if img.width().max(img.height()) > side * 2 {
            let (w, h) = (img.width(), img.height());
            let f = (side * 2) as f64 / w.max(h) as f64;
            return Some(image::imageops::resize(
                &img,
                ((w as f64 * f).round() as u32).max(1),
                ((h as f64 * f).round() as u32).max(1),
                image::imageops::FilterType::Triangle,
            ));
        }
        Some(img)
    }
}
