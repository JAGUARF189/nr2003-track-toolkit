//! PKWare Data Compression Library "implode" decompressor (a.k.a. "explode").
//!
//! NR2003 mips store some mip levels DCL-compressed (the BMHD "compressed" flag).
//! Written from the published format description (the same one Mark Adler's
//! blast.c implements): a 2-byte header (literal mode, dictionary size), then an
//! LSB-first bit stream of literals and (length, distance) copies, ended by
//! length code 519. Huffman codes are canonical with their bits inverted.

use anyhow::{bail, Result};

const MAXBITS: usize = 13;

struct Huffman {
    count: [u16; MAXBITS + 1],
    symbol: Vec<u16>,
}

impl Huffman {
    /// Build from run-length-compressed code lengths: each byte is
    /// `(repeat - 1) << 4 | length`.
    fn new(rep: &[u8], n: usize) -> Self {
        let mut lengths = Vec::with_capacity(n);
        for &b in rep {
            let len = b & 15;
            for _ in 0..(b >> 4) + 1 {
                lengths.push(len);
            }
        }
        assert_eq!(lengths.len(), n, "DCL code table expands to the wrong size");
        let mut count = [0u16; MAXBITS + 1];
        for &l in &lengths {
            count[l as usize] += 1;
        }
        let mut offs = [0u16; MAXBITS + 1];
        for len in 1..MAXBITS {
            offs[len + 1] = offs[len] + count[len];
        }
        let mut symbol = vec![0u16; n];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbol[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        Self { count, symbol }
    }
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    cnt: u32,
}

impl Bits<'_> {
    fn need(&mut self, n: u32) -> Result<u32> {
        while self.cnt < n {
            let Some(&b) = self.data.get(self.pos) else {
                bail!("DCL stream ended early");
            };
            self.pos += 1;
            self.buf |= (b as u32) << self.cnt;
            self.cnt += 8;
        }
        let v = self.buf & ((1u32 << n) - 1);
        self.buf >>= n;
        self.cnt -= n;
        Ok(v)
    }

    fn decode(&mut self, h: &Huffman) -> Result<usize> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..=MAXBITS {
            code |= (self.need(1)? ^ 1) as i32; // codes are stored inverted
            let count = h.count[len] as i32;
            if code < first + count {
                return Ok(h.symbol[(index + code - first) as usize] as usize);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        bail!("invalid DCL Huffman code")
    }
}

const LITLEN: [u8; 98] = [
    11, 124, 8, 7, 28, 7, 188, 13, 76, 4, 10, 8, 12, 10, 12, 10, 8, 23, 8, 9, 7, 6, 7, 8, 7, 6, 55,
    8, 23, 24, 12, 11, 7, 9, 11, 12, 6, 7, 22, 5, 7, 24, 6, 11, 9, 6, 7, 22, 7, 11, 38, 7, 9, 8,
    25, 11, 8, 11, 9, 12, 8, 12, 5, 38, 5, 38, 5, 11, 7, 5, 6, 21, 6, 10, 53, 8, 7, 24, 10, 27, 44,
    253, 253, 253, 252, 252, 252, 13, 12, 45, 12, 45, 12, 61, 12, 45, 44, 173,
];
const LENLEN: [u8; 6] = [2, 35, 36, 53, 38, 23];
const DISTLEN: [u8; 7] = [2, 20, 53, 230, 247, 151, 248];
const BASE: [u16; 16] = [3, 2, 4, 5, 6, 7, 8, 9, 10, 12, 16, 24, 40, 72, 136, 264];
const EXTRA: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8];

struct Tables {
    lit: Huffman,
    len: Huffman,
    dist: Huffman,
}

fn tables() -> &'static Tables {
    use std::sync::OnceLock;
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| Tables {
        lit: Huffman::new(&LITLEN, 256),
        len: Huffman::new(&LENLEN, 16),
        dist: Huffman::new(&DISTLEN, 64),
    })
}

/// Decompress a complete DCL stream. `expected` is used as a capacity hint and
/// sanity limit (the output may not grow past 4x it).
pub fn explode(input: &[u8], expected: usize) -> Result<Vec<u8>> {
    if input.len() < 2 {
        bail!("DCL stream too short");
    }
    let lit_mode = input[0];
    let dict = input[1] as u32;
    if lit_mode > 1 {
        bail!("bad DCL literal mode {lit_mode}");
    }
    if !(4..=6).contains(&dict) {
        bail!("bad DCL dictionary size {dict}");
    }
    let t = tables();
    let mut bits = Bits {
        data: input,
        pos: 2,
        buf: 0,
        cnt: 0,
    };
    let mut out: Vec<u8> = Vec::with_capacity(expected);
    let limit = expected.saturating_mul(4).max(1 << 16);
    loop {
        if bits.need(1)? == 1 {
            let sym = bits.decode(&t.len)?;
            let len = BASE[sym] as usize + bits.need(EXTRA[sym] as u32)? as usize;
            if len == 519 {
                break;
            }
            let shift = if len == 2 { 2 } else { dict };
            let mut dist = (bits.decode(&t.dist)? << shift) as usize;
            dist += bits.need(shift)? as usize;
            dist += 1;
            if dist > out.len() {
                bail!("DCL copy distance {dist} reaches before the start of the output");
            }
            let start = out.len() - dist;
            for i in 0..len {
                let b = out[start + i];
                out.push(b);
            }
        } else {
            let b = if lit_mode == 1 {
                bits.decode(&t.lit)? as u8
            } else {
                bits.need(8)? as u8
            };
            out.push(b);
        }
        if out.len() > limit {
            bail!("DCL output exceeds the expected size");
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_expand() {
        let _ = tables(); // asserts the code tables expand to 256/16/64 entries
    }

    #[test]
    fn blast_reference_vector() {
        // The canonical example from the format description: decodes to "AIAIAIAIAIAIA".
        let input = [0x00, 0x04, 0x82, 0x24, 0x25, 0x8f, 0x80, 0x7f];
        assert_eq!(explode(&input, 13).unwrap(), b"AIAIAIAIAIAIA");
    }
}
