//! Read NR2003 `.dat` archives (the files `mklib` builds) without unpacking.
//!
//! Layout (verified against mklib -u output for hillside.dat and shared.dat):
//!
//! ```text
//! 0x00 u32  checksum/stamp (differs per file)
//! 0x04 u32  0
//! 0x08 u16  0
//! 0x0A      index entries, back to back, until the first file's data:
//!           u8 5, u8 flags (0 = stored, 2 = PKWare DCL compressed),
//!           u32 size, u32 stored size, u32 offset, u8 name length, name, NUL
//!           names use backslashes, e.g. "trackmat\apron.mip"
//! then      file data, contiguous in index order, to the end of the file
//! ```

use crate::mip::dcl;
use crate::util;
use anyhow::{bail, Context, Result};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug)]
pub struct DatEntry {
    /// As stored, with backslashes (e.g. `trackmat\apron.mip`).
    pub name: String,
    pub size: u32,
    pub stored: u32,
    pub offset: u32,
    pub compressed: bool,
}

impl DatEntry {
    /// File name without any folder.
    pub fn file_name(&self) -> &str {
        self.name.rsplit(['\\', '/']).next().unwrap_or(&self.name)
    }

    pub fn ext_lower(&self) -> String {
        self.file_name()
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default()
    }

    /// Safe relative path for extraction: rejects absolute paths and `..`.
    pub fn relative_path(&self) -> Result<PathBuf> {
        let p = PathBuf::from(self.name.replace('/', "\\"));
        for c in p.components() {
            match c {
                Component::Normal(_) => {}
                _ => bail!("unsafe path in archive: {}", self.name),
            }
        }
        Ok(p)
    }
}

pub struct Dat {
    pub path: PathBuf,
    bytes: Vec<u8>,
    pub entries: Vec<DatEntry>,
}

impl Dat {
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let entries = Self::parse_index(&bytes)
            .with_context(|| format!("{} isn't a readable .dat", util::file_name(path)))?;
        Ok(Self {
            path: path.to_path_buf(),
            bytes,
            entries,
        })
    }

    fn parse_index(b: &[u8]) -> Result<Vec<DatEntry>> {
        if b.len() < 10 {
            bail!("file too small");
        }
        let u32_at = |o: usize| -> Result<u32> {
            b.get(o..o + 4)
                .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
                .context("index runs past the end of the file")
        };
        let mut pos = 10usize;
        let mut first_data = b.len();
        let mut entries = Vec::new();
        while pos < first_data {
            let marker = *b.get(pos).context("index runs past the end of the file")?;
            if marker != 5 {
                bail!("unexpected index byte {marker:#x} at 0x{pos:X}");
            }
            let flags = b[pos + 1];
            let size = u32_at(pos + 2)?;
            let stored = u32_at(pos + 6)?;
            let offset = u32_at(pos + 10)?;
            let nlen = *b
                .get(pos + 14)
                .context("index runs past the end of the file")? as usize;
            let name_bytes = b
                .get(pos + 15..pos + 15 + nlen)
                .context("index runs past the end of the file")?;
            let name = String::from_utf8_lossy(name_bytes).into_owned();
            if offset as usize + stored as usize > b.len() {
                bail!("{name} runs past the end of the file");
            }
            if flags != 0 && flags != 2 {
                bail!("{name} has unknown storage flags {flags}");
            }
            first_data = first_data.min(offset as usize);
            entries.push(DatEntry {
                name,
                size,
                stored,
                offset,
                compressed: flags == 2,
            });
            pos += 15 + nlen + 1;
            if entries.len() > 200_000 {
                bail!("implausible number of entries");
            }
        }
        if entries.is_empty() {
            bail!("empty index");
        }
        Ok(entries)
    }

    /// The contents of one entry, decompressed.
    pub fn read(&self, e: &DatEntry) -> Result<Vec<u8>> {
        let raw = &self.bytes[e.offset as usize..e.offset as usize + e.stored as usize];
        let data = if e.compressed {
            dcl::explode(raw, e.size as usize)
                .with_context(|| format!("decompressing {}", e.name))?
        } else {
            raw.to_vec()
        };
        if data.len() != e.size as usize {
            bail!(
                "{} came out as {} bytes, expected {}",
                e.name,
                data.len(),
                e.size
            );
        }
        Ok(data)
    }

    pub fn find(&self, name: &str) -> Option<&DatEntry> {
        self.entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
    }

    /// Write one entry under `dir`, keeping its sub-folder (e.g. `trackmat\`).
    /// Returns the written path.
    pub fn extract(&self, e: &DatEntry, dir: &Path) -> Result<PathBuf> {
        let out = dir.join(e.relative_path()?);
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&out, self.read(e)?)
            .with_context(|| format!("writing {}", out.display()))?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> DatEntry {
        DatEntry {
            name: name.into(),
            size: 0,
            stored: 0,
            offset: 0,
            compressed: false,
        }
    }

    #[test]
    fn safe_paths() {
        assert_eq!(
            entry("trackmat\\apron.mip").relative_path().unwrap(),
            PathBuf::from("trackmat\\apron.mip")
        );
        assert!(entry("..\\evil.mip").relative_path().is_err());
        assert!(entry("C:\\evil.mip").relative_path().is_err());
        assert!(entry("\\evil.mip").relative_path().is_err());
        assert_eq!(entry("trackmat\\apron.mip").file_name(), "apron.mip");
        assert_eq!(entry("trackmat\\apron.MIP").ext_lower(), "mip");
    }

    #[test]
    fn parse_minimal() {
        // header, one stored entry "a.mip" with 3 bytes of data
        let mut b = vec![0u8; 10];
        let name = b"a.mip";
        let data_off = 10 + 15 + name.len() + 1;
        b.push(5);
        b.push(0);
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&(data_off as u32).to_le_bytes());
        b.push(name.len() as u8);
        b.extend_from_slice(name);
        b.push(0);
        b.extend_from_slice(b"xyz");
        let entries = Dat::parse_index(&b).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "a.mip");
        assert_eq!(entries[0].offset as usize, data_off);
    }
}
