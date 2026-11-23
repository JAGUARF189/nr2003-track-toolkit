//! NR2003 `.mip` texture reader.
//!
//! Layout (reverse-engineered; see docs/mip-format.md):
//!
//! ```text
//! 0x00 u16  version (4)
//! 0x02 u8   type      3 RGB565, 4 ARGB1555, 5 ARGB4444, 6 R,G,B bytes, 7 B,G,R,A bytes, 10 DXT1, 11 DXT3
//!                     (all verified against makemip output, see `ttk mipdiff`)
//! 0x03 u8   mapping   0 tile, 1 repeat vertically, 2 repeat horizontally, 3 stretch
//! 0x04 u32  log2(width)
//! 0x08 u32  log2(height)
//! 0x0C u8×4 average color, B G R A
//! 0x10 u32  number of mip levels
//! 0x14 u32  ?
//! 0x18 f32  priority (WinMip2: 0.1..4.0)
//! 0x1C u32  ?
//! 0x20      records: tag (4 bytes, stored reversed), u32 0, u32 size, data padded to 4 with spaces
//!           BMAP { BMHD { u8 type, u32 w, u32 h, u32 pitch, u8 compressed }, DATA { pixels } }
//!           one BMAP per level, smallest first
//! ```
//! Compressed DATA is a PKWare DCL stream ([`dcl::explode`]).

pub mod build;
pub mod dcl;

use anyhow::{bail, Context, Result};
use image::{Rgba, RgbaImage};
use std::borrow::Cow;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct MipInfo {
    pub version: u16,
    pub kind: u8,
    pub mapping: u8,
    pub width: u32,
    pub height: u32,
    pub levels_declared: u32,
    /// B, G, R, A
    pub average_bgra: [u8; 4],
    pub priority: f32,
    /// A `.stp` menu image (STMP container, any size, no mip levels).
    pub stp: bool,
}

#[derive(Clone, Debug)]
pub struct Level {
    pub width: u32,
    pub height: u32,
    pub kind: u8,
    pub pitch: u32,
    pub compressed: bool,
    /// Offset and length of the DATA payload in the file.
    pub data_offset: usize,
    pub data_len: usize,
}

pub struct Mip {
    pub info: MipInfo,
    /// Smallest first, as stored.
    pub levels: Vec<Level>,
    bytes: Vec<u8>,
}

fn u32_at(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .with_context(|| format!("file ends inside the field at 0x{o:X}"))
}

fn tag_at(b: &[u8], o: usize) -> Option<[u8; 4]> {
    let s = b.get(o..o + 4)?;
    Some([s[3], s[2], s[1], s[0]])
}

/// Bytes per 4×4 block for the compressed types.
fn block_bytes(kind: u8) -> Option<usize> {
    match kind {
        10 => Some(8),
        11 => Some(16),
        _ => None,
    }
}

fn bytes_per_pixel(kind: u8) -> Option<usize> {
    match kind {
        3 | 4 | 5 => Some(2),
        6 => Some(3),
        7 => Some(4),
        _ => None,
    }
}

impl Level {
    /// Size of the pixel data once decompressed.
    pub fn expected_len(&self) -> Result<usize> {
        let (w, h) = (self.width as usize, self.height as usize);
        if let Some(bb) = block_bytes(self.kind) {
            Ok(w.div_ceil(4).max(1) * h.div_ceil(4).max(1) * bb)
        } else if bytes_per_pixel(self.kind).is_some() {
            Ok(self.pitch as usize * h)
        } else {
            bail!("unsupported mip type {}", self.kind)
        }
    }
}

impl Mip {
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(bytes)
    }

    pub fn parse(bytes: Vec<u8>) -> Result<Self> {
        if tag_at(&bytes, 0).as_ref() == Some(b"STMP") {
            return Self::parse_stp(bytes);
        }
        if bytes.len() < 32 {
            bail!("too small to be a mip ({} bytes)", bytes.len());
        }
        let version = u16::from_le_bytes([bytes[0], bytes[1]]);
        let kind = bytes[2];
        let mapping = bytes[3];
        let lw = u32_at(&bytes, 4)?;
        let lh = u32_at(&bytes, 8)?;
        if lw > 15 || lh > 15 {
            bail!("not an NR2003 mip (size field 2^{lw} x 2^{lh})");
        }
        let info = MipInfo {
            version,
            kind,
            mapping,
            width: 1 << lw,
            height: 1 << lh,
            levels_declared: u32_at(&bytes, 0x10)?,
            average_bgra: [bytes[12], bytes[13], bytes[14], bytes[15]],
            priority: f32::from_le_bytes([bytes[0x18], bytes[0x19], bytes[0x1A], bytes[0x1B]]),
            stp: false,
        };

        let mut levels = Vec::new();
        // Stop at the declared level count: some tools overwrote mips without
        // truncating, leaving stale data after the real levels. The game ignores it.
        let limit = match info.levels_declared {
            0 => usize::MAX,
            n => n as usize,
        };
        Self::collect_bmaps(&bytes, 0x20, bytes.len(), &mut levels, limit)?;
        if levels.is_empty() {
            bail!("no image data (no BMAP records)");
        }
        Ok(Self {
            info,
            levels,
            bytes,
        })
    }

    /// Walk records in `start..end`, collecting BMAPs (descending into STMP).
    fn collect_bmaps(
        b: &[u8],
        start: usize,
        end: usize,
        levels: &mut Vec<Level>,
        limit: usize,
    ) -> Result<()> {
        let mut pos = start;
        while pos + 12 <= end && levels.len() < limit {
            let tag = tag_at(b, pos).unwrap();
            let size = u32_at(b, pos + 8)? as usize;
            let body = pos + 12;
            if body + size > end {
                bail!("record at 0x{pos:X} runs past its end");
            }
            match &tag {
                b"BMAP" => levels.push(
                    Self::parse_bmap(b, body, body + size)
                        .with_context(|| format!("BMAP at 0x{pos:X}"))?,
                ),
                b"STMP" => Self::collect_bmaps(b, body, body + size, levels, limit)?,
                _ => {}
            }
            pos = body + ((size + 3) & !3);
        }
        Ok(())
    }

    /// `.stp` menu image: STMP { STHD, DATA (hot x/y, width, height), BMAP… }.
    fn parse_stp(bytes: Vec<u8>) -> Result<Self> {
        let mut levels = Vec::new();
        Self::collect_bmaps(&bytes, 0, bytes.len(), &mut levels, usize::MAX)?;
        if levels.is_empty() {
            bail!("no image data in this .stp");
        }
        let big = levels
            .iter()
            .max_by_key(|l| l.width as u64 * l.height as u64)
            .unwrap()
            .clone();
        let info = MipInfo {
            version: 0,
            kind: big.kind,
            mapping: 3,
            width: big.width,
            height: big.height,
            levels_declared: levels.len() as u32,
            average_bgra: [0; 4],
            priority: 0.0,
            stp: true,
        };
        Ok(Self {
            info,
            levels,
            bytes,
        })
    }

    fn parse_bmap(b: &[u8], start: usize, end: usize) -> Result<Level> {
        let mut hd: Option<(u8, u32, u32, u32, bool)> = None;
        let mut data: Option<(usize, usize)> = None;
        let mut pos = start;
        while pos + 12 <= end {
            let tag = tag_at(b, pos).unwrap();
            let size = u32_at(b, pos + 8)? as usize;
            let body = pos + 12;
            if body + size > end {
                bail!("record at 0x{pos:X} overflows its BMAP");
            }
            match &tag {
                b"BMHD" => {
                    if size < 14 {
                        bail!("short BMHD");
                    }
                    hd = Some((
                        b[body],
                        u32_at(b, body + 1)?,
                        u32_at(b, body + 5)?,
                        u32_at(b, body + 9)?,
                        b[body + 13] != 0,
                    ));
                }
                b"DATA" => data = Some((body, size)),
                _ => {}
            }
            pos = body + ((size + 3) & !3);
        }
        let (kind, width, height, pitch, compressed) = hd.context("BMAP without BMHD")?;
        let (data_offset, data_len) = data.context("BMAP without DATA")?;
        if width == 0 || height == 0 || width > 32768 || height > 32768 {
            bail!("bad level size {width}x{height}");
        }
        Ok(Level {
            width,
            height,
            kind,
            pitch,
            compressed,
            data_offset,
            data_len,
        })
    }

    pub fn largest_index(&self) -> usize {
        (0..self.levels.len())
            .max_by_key(|&i| self.levels[i].width as u64 * self.levels[i].height as u64)
            .unwrap()
    }

    /// Smallest level that is at least `min_side` on its longer side (or the largest).
    pub fn level_for(&self, min_side: u32) -> usize {
        let mut best = self.largest_index();
        for (i, l) in self.levels.iter().enumerate() {
            let side = l.width.max(l.height);
            let best_side = self.levels[best].width.max(self.levels[best].height);
            if side >= min_side && side < best_side {
                best = i;
            }
        }
        best
    }

    /// Pixel data of a level, decompressed if needed.
    pub fn level_data(&self, i: usize) -> Result<Cow<'_, [u8]>> {
        let l = &self.levels[i];
        let raw = &self.bytes[l.data_offset..l.data_offset + l.data_len];
        let need = l.expected_len()?;
        let data: Cow<[u8]> = if l.compressed {
            Cow::Owned(
                dcl::explode(raw, need)
                    .with_context(|| format!("decompressing {}x{} level", l.width, l.height))?,
            )
        } else {
            Cow::Borrowed(raw)
        };
        if data.len() < need {
            bail!(
                "{}x{} level has {} bytes of pixels, needs {}",
                l.width,
                l.height,
                data.len(),
                need
            );
        }
        Ok(data)
    }

    pub fn decode_level(&self, i: usize) -> Result<RgbaImage> {
        let l = &self.levels[i];
        let data = self.level_data(i)?;
        decode_pixels(l, &data)
    }

    pub fn decode_largest(&self) -> Result<RgbaImage> {
        self.decode_level(self.largest_index())
    }
}

fn expand5(v: u16) -> u8 {
    ((v << 3) | (v >> 2)) as u8
}
fn expand6(v: u16) -> u8 {
    ((v << 2) | (v >> 4)) as u8
}
fn expand4(v: u16) -> u8 {
    (v * 17) as u8
}

fn rgb565(c: u16) -> [u8; 3] {
    [
        expand5((c >> 11) & 31),
        expand6((c >> 5) & 63),
        expand5(c & 31),
    ]
}

fn decode_pixels(l: &Level, d: &[u8]) -> Result<RgbaImage> {
    let (w, h) = (l.width, l.height);
    let mut img = RgbaImage::new(w, h);
    match l.kind {
        3 | 4 | 5 | 6 | 7 => {
            let bpp = bytes_per_pixel(l.kind).unwrap();
            let pitch = (l.pitch as usize).max(w as usize * bpp);
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let o = y * pitch + x * bpp;
                    let px = match l.kind {
                        3 => {
                            let c = u16::from_le_bytes([d[o], d[o + 1]]);
                            let [r, g, b] = rgb565(c);
                            [r, g, b, 255]
                        }
                        4 => {
                            let c = u16::from_le_bytes([d[o], d[o + 1]]);
                            let a = if c & 0x8000 != 0 { 255 } else { 0 };
                            [
                                expand5((c >> 10) & 31),
                                expand5((c >> 5) & 31),
                                expand5(c & 31),
                                a,
                            ]
                        }
                        5 => {
                            let c = u16::from_le_bytes([d[o], d[o + 1]]);
                            [
                                expand4((c >> 8) & 15),
                                expand4((c >> 4) & 15),
                                expand4(c & 15),
                                expand4(c >> 12),
                            ]
                        }
                        6 => [d[o], d[o + 1], d[o + 2], 255], // R G B (unlike type 7's B G R A)
                        _ => [d[o + 2], d[o + 1], d[o], d[o + 3]],
                    };
                    img.put_pixel(x as u32, y as u32, Rgba(px));
                }
            }
        }
        10 | 11 => {
            let bb = block_bytes(l.kind).unwrap();
            let bw = (w as usize).div_ceil(4).max(1);
            let bh = (h as usize).div_ceil(4).max(1);
            for by in 0..bh {
                for bx in 0..bw {
                    let o = (by * bw + bx) * bb;
                    let blk = &d[o..o + bb];
                    let (alpha, color) = if l.kind == 11 {
                        (Some(&blk[0..8]), &blk[8..16])
                    } else {
                        (None, &blk[0..8])
                    };
                    let c0 = u16::from_le_bytes([color[0], color[1]]);
                    let c1 = u16::from_le_bytes([color[2], color[3]]);
                    let (p0, p1) = (rgb565(c0), rgb565(c1));
                    let mix = |a: [u8; 3], b: [u8; 3], wa: u16, wb: u16| -> [u8; 3] {
                        let f =
                            |i: usize| ((a[i] as u16 * wa + b[i] as u16 * wb) / (wa + wb)) as u8;
                        [f(0), f(1), f(2)]
                    };
                    // DXT1 with c0 <= c1 has a transparent 4th color; DXT3 always uses 4-color mode.
                    let four = c0 > c1 || l.kind == 11;
                    let palette: [[u8; 4]; 4] = if four {
                        let p2 = mix(p0, p1, 2, 1);
                        let p3 = mix(p0, p1, 1, 2);
                        [
                            [p0[0], p0[1], p0[2], 255],
                            [p1[0], p1[1], p1[2], 255],
                            [p2[0], p2[1], p2[2], 255],
                            [p3[0], p3[1], p3[2], 255],
                        ]
                    } else {
                        let p2 = mix(p0, p1, 1, 1);
                        [
                            [p0[0], p0[1], p0[2], 255],
                            [p1[0], p1[1], p1[2], 255],
                            [p2[0], p2[1], p2[2], 255],
                            [0, 0, 0, 0],
                        ]
                    };
                    let idx = u32::from_le_bytes([color[4], color[5], color[6], color[7]]);
                    for py in 0..4usize {
                        for px in 0..4usize {
                            let x = bx * 4 + px;
                            let y = by * 4 + py;
                            if x >= w as usize || y >= h as usize {
                                continue;
                            }
                            let k = py * 4 + px;
                            let mut c = palette[((idx >> (2 * k)) & 3) as usize];
                            if let Some(a) = alpha {
                                let nib = (a[k / 2] >> ((k % 2) * 4)) & 15;
                                c[3] = nib * 17;
                            }
                            img.put_pixel(x as u32, y as u32, Rgba(c));
                        }
                    }
                }
            }
        }
        k => bail!("unsupported mip type {k}"),
    }
    Ok(img)
}

/// Plain-English descriptions for the UI.
pub fn type_name(kind: u8) -> &'static str {
    match kind {
        3 => "Solid colors (16-bit)",
        4 => "Has one see-through color",
        5 => "Soft see-through parts (16 levels)",
        6 => "Solid colors (full quality)",
        7 => "Smooth see-through parts (full quality)",
        10 => "Compressed",
        11 => "See-through parts (compressed)",
        _ => "Unknown kind",
    }
}

pub fn mapping_name(m: u8) -> &'static str {
    match m {
        0 => "Repeats like floor tiles",
        1 => "Repeats up and down",
        2 => "Repeats side to side",
        3 => "Stretched to fit",
        _ => "Unknown",
    }
}

/// What the pixels actually do (type 10 can punch fully transparent holes even
/// though it's the "solid" compressed type, so the type alone isn't enough).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transparency {
    None,
    /// Only fully clear or fully solid pixels (cut-outs like fences)
    Holes,
    /// Partly see-through pixels (glass, shadows, soft edges)
    Soft,
}

impl Transparency {
    pub fn of(img: &RgbaImage) -> Self {
        let mut holes = false;
        for p in img.pixels() {
            match p.0[3] {
                255 => {}
                0 => holes = true,
                _ => return Transparency::Soft,
            }
        }
        if holes {
            Transparency::Holes
        } else {
            Transparency::None
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Transparency::None => "No",
            Transparency::Holes => "Yes, some parts are cut out",
            Transparency::Soft => "Yes, some parts are see-through",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a one-level mip: header + BMAP{BMHD, DATA}.
    fn make(kind: u8, w: u32, h: u32, pitch: u32, data: &[u8]) -> Vec<u8> {
        let mut f = vec![0u8; 32];
        f[0] = 4;
        f[2] = kind;
        f[3] = 3;
        f[4..8].copy_from_slice(&w.trailing_zeros().to_le_bytes());
        f[8..12].copy_from_slice(&h.trailing_zeros().to_le_bytes());
        f[0x10..0x14].copy_from_slice(&1u32.to_le_bytes());
        f[0x18..0x1C].copy_from_slice(&0.1f32.to_le_bytes());
        let rec = |tag: &[u8; 4], body: &[u8]| {
            let mut r = vec![tag[3], tag[2], tag[1], tag[0], 0, 0, 0, 0];
            r.extend_from_slice(&(body.len() as u32).to_le_bytes());
            r.extend_from_slice(body);
            while r.len() % 4 != 0 {
                r.push(b' ');
            }
            r
        };
        let mut hd = vec![kind];
        hd.extend_from_slice(&w.to_le_bytes());
        hd.extend_from_slice(&h.to_le_bytes());
        hd.extend_from_slice(&pitch.to_le_bytes());
        hd.push(0);
        let mut inner = rec(b"BMHD", &hd);
        inner.extend(rec(b"DATA", data));
        f.extend(rec(b"BMAP", &inner));
        f
    }

    #[test]
    fn rgb565_two_by_two() {
        // red, green / blue, white  (row pitch 4)
        let data = [0x00, 0xF8, 0xE0, 0x07, 0x1F, 0x00, 0xFF, 0xFF];
        let m = Mip::parse(make(3, 2, 2, 4, &data)).unwrap();
        assert_eq!((m.info.width, m.info.height, m.levels.len()), (2, 2, 1));
        let img = m.decode_largest().unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(1, 0).0, [0, 255, 0, 255]);
        assert_eq!(img.get_pixel(0, 1).0, [0, 0, 255, 255]);
        assert_eq!(img.get_pixel(1, 1).0, [255, 255, 255, 255]);
        assert_eq!(Transparency::of(&img), Transparency::None);
    }

    #[test]
    fn dxt1_punch_through() {
        // c0 <= c1 selects 3-color mode; index 3 is transparent. All pixels index 3.
        let block = [0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        let img = Mip::parse(make(10, 4, 4, 2, &block))
            .unwrap()
            .decode_largest()
            .unwrap();
        assert_eq!(img.get_pixel(2, 2).0[3], 0);
        assert_eq!(Transparency::of(&img), Transparency::Holes);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Mip::parse(vec![0u8; 10]).is_err());
        assert!(Mip::parse(vec![0xFFu8; 64]).is_err());
    }
}
