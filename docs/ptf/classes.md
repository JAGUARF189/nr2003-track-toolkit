# .ptf classes: full field layouts (decoded 2026-11-23)

These are all 11 classes Sandbox writes. They were decoded from the sandbox.exe `transfer()` routines; `re/find_classes.py` lists the addresses. They were then **verified read-for-read against Sandbox's own code running in an emulator** (`re/ptf_emu.py`).

- **Proof:** 8 tracks (hillside, martinsville, talladega, watkins_glen, bristol_night, dpark, Kil Kare night, sonoma_12, Kansas_20) gave 1.24 M reads, all identical.
- **Coverage:** the prototype reader `re/ptf_read.py` reads all 55 .ptf files on this PC to the end. 4 extracted copies have one stray trailing `00` byte, which must be preserved.

**Notation:**
- Every class starts with the Descriptor base (`i32 1`, `string name`), then `i32 version`. Only the versions below occur in real files.
- `obj` means a `transferPersistentObject` reference (see archive-format.md).
- `list(n)` means: if n > 0, `i32 n*4` (transferPointerButNotData), then n × obj.
- `ptr` means transferPointer: `i32 byteSize` (0 = null) + bytes.
- `+0xNN` is the member offset in Sandbox's in-memory object.

## TrackDescriptor (v8): see TrackDescriptor.md
`i32 segCount, u8 night, f64 lighting[3], u32 unk2C, i32 nRec, [i32 n*48, n×48 bytes], obj extra, i32 nDetails + list, i32 nTsoRefs + list, i32 segCount*4, segCount × obj Segment`

## TrackDetailDescriptor (v0)
`obj Appearance, f64 +10, f64 +18, f64 +28, f64 +20, f64 +30, ptr +38, ptr +3C`

## AppearanceDescriptor (v2)
- `7 × obj Texture`: slots +0C +10 +14 +18 +1C +20 +24.
- `f64[3] +28, f64[3] +40, f64[3] +58`: 1,1,1 / 1,1,1 / 0,0,0 in stock files, likely colour or scale.
- `f64 +70, f64 +78, f64 +80, i32 +88`

## TextureDescriptor (v2)
`string file` (e.g. `trackmat\asphalt.mip`), `u8 flag`. On load Sandbox opens the .mip (vtable slot 6, `004893E0`).

## TSODescriptor (v3)
`string file` (e.g. `bldg1.3do`), `string +10`, `u8 flag`. On load Sandbox opens the .3do (`0047AD00`).

## TSOReferenceDescriptor (v6): one placed object
- `f64 x, y, z, rot1, rot2, rot3`: rot1 is −π..π (heading), rot2 and rot3 are small (pitch and roll?). x and y are world metres.
- `obj TSODescriptor, i32 +40`
- `u8 noExtra`. If 0: `32 bytes, i32, i32, i32` (a 44-byte block at +3C).
- `i32 n; n×40 bytes` (+48/+44)
- `string sound` (e.g. `blimp.wav`)

## SegmentDescriptor (v2): one piece of centerline
- `i32 nX + list(X_Section)`, `i32 nF + list(F_Section)`, `i32 nW + list(W_Section)`
- `i32 type`: 0 = straight (Sandbox checks that the heading matches the chord), 1 = curve.
- `f64 startX, startY, startHeading, endX, endY, endHeading`: metres, radians, heading 0 = +X.
- `u8 +40, u8 +41`
- `i32 n; [i32 n*12, n×12 bytes]`, `i32 m; [i32 m*16, m×16 bytes]`: 0 in stock files.

## X_SectionDescriptor (v3): cross-section height profile point (CenterLine "x section")
- `f32 dlatStart, f64 altitudeStart, f32 slopeStart, f32 dlatEnd, f64 altitudeEnd, f32 slopeEnd`
- `u8 +14`
- Twice: `u8 mode`. If mode ≠ 0: `f32, i32`. If mode == 2: also `u8, u8`.

## F_SectionDescriptor (v5): ground strip boundary (CenterLine "f section")
- `f32 dlatStart, f32 dlatEnd, u8 +14, i32 +19, i32 +15` (one of the two ints is the surface type)
- `obj Appearance`. If it is null, or all of its slots +0C +10 +14 +18 +20 are null, stop here.
- `obj TextureCoords`
- `6 × layer`. Layer k is present only if its Appearance slot is non-null; slot order is k → +0C, +10, +18, +14, +1C, +20.
  - **F layer:** `u8 shared`. If 0: `2 × (u8 mode, f64 texel)`, then `3 × f64`.

## W_SectionDescriptor (v7): wall (CenterLine "w section")
- `f32 dlatStart, f32 dlatEnd, u8 +14, i32 +55`
- `f64 heightStart, heightEnd, widthStart, widthEnd, +35, +3D` (+35/+3D are likely the physical width)
- `i32 +45, i32 +49`. If +49 == 1: 40 bytes.
- `i32 wallType` (2049 hard, 2053/2058 soft, … see HANDOFF)
- `i32 nSub`, then for each subsection: `f64 +28`, then 5 faces (Right, Top, Left, Near, Far). Each face:
  - `obj Appearance`. If it is null or has no texture: nothing more.
  - Otherwise `obj TextureCoords` and `6 × layer` (same slot rule as F).
  - **W layer:** `u8 shared`. If 0: `2 × (u8 mode, f64, f64)`.

## TextureCoordsDescriptor (v1)
`i32 count` (always 4), `i32 subVersion` (3), `24 × ptr` (count doubles each, or null).

## Not serialized
Appearance `+9C` is never set while loading; it is assumed 0, so layer 2 is always read when its slot is set.
