# NR2003 .mip format

The reader is in `src/mip/` (Rust). It has been verified two ways:

- All 983 stock and mod mips on this PC decode, 8,870 levels in total, 862 of them DCL-compressed. Each compressed level decompresses to exactly its expected size.
- makemip output in every mode decodes to the original pixels, within each format's precision. Run `ttk mipdiff <mip> <source.tga>` to repeat the check.

## Header (32 bytes)

| Off | Type | Field |
|---|---|---|
| 0x00 | u16 | Version (4) |
| 0x02 | u8 | Type. See the table below. |
| 0x03 | u8 | Mapping: 0 tile, 1 repeat vertically, 2 repeat horizontally, 3 stretch. The 3do must agree. makemip writes 0 with `-tile` and 3 by default. |
| 0x04 | u32 | log2(width) |
| 0x08 | u32 | log2(height) |
| 0x0C | u8×4 | Average color, stored B G R A. Only filled for uncompressed types (`00 00 00 FF` otherwise). |
| 0x10 | u32 | Number of mip levels |
| 0x14 | u32 | ? (0) |
| 0x18 | f32 | Priority, 0.1 to 4.0. WinMip shows it ×10. Stock trackmat textures use 0.3/0.4, objects 0.1. |
| 0x1C | u32 | ? (0) |

## Records (from 0x20 to the end of the file)

Each record is `tag[4]` (stored reversed, so `PAMB` on disk is BMAP), then `u32 0`, then `u32 size`. The data follows, padded to a multiple of 4 with spaces (`0x20`).

There is one `BMAP` per mip level, stored **smallest level first**:

```
BMAP
  BMHD  u8 type, u32 width, u32 height, u32 rowPitch, u8 compressed    (14 bytes)
  DATA  pixels; a PKWare DCL "implode" stream when compressed = 1 (header 00 06 = binary mode, 4 KB dictionary)
```

## Pixel types

| Type | Format | makemip flags |
|---|---|---|
| 3 | RGB565 little-endian | opaque, default |
| 4 | ARGB1555 | `-trans` |
| 5 | ARGB4444 | alpha, default |
| 6 | 3 bytes **R G B** | opaque `-trueColor` |
| 7 | 4 bytes **B G R A** | alpha `-trueColor` |
| 10 | DXT1. With c0 <= c1, index 3 is a transparent pixel, so "solid" mips can still have cut-out holes. | `-dxt` |
| 11 | DXT3 (explicit 4-bit alpha) | alpha `-dxt` |

## Reference

The WinMip2 help file (`WinMip 2 Help.htm`) gives the type list and the mapping and priority meanings.
