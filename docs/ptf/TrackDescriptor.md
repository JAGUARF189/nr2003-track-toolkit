# TrackDescriptor (root object of every .ptf)

These are the sandbox.exe addresses (the image base is `0x400000`). The game exe contains the same class, which hasn't been checked yet.

| What | Address |
|---|---|
| Class registration | `0048B270`: `PersistentLoadingInfo{ "TrackDescriptor", factory 0048B230 }` |
| Factory | `0048B230`: `_memAlloc(&p, 0x51)`, then the constructor at `0048B6B0` |
| vtable | `004AD0E0`. Slot 0 = **transfer** `0048BAA0`, slot 1 = `getClassName` `0048F560`, slot 6 = `0048C320` (also uses objects, role unknown) |
| Base transfer | `00486BB0` (Descriptor) |

## In-memory layout (packed, 0x51 bytes)

`cpp/src/layout/track_descriptor_mem.hpp` contains this layout with `static_assert`s.

| Off | Size | Field | Notes |
|---|---|---|---|
| 0x00 | 4 | vtable | |
| 0x04 | 4 | refCount | PersistentObject |
| 0x08 | 4 | name (char*) | Descriptor |
| 0x0C | 4 | segments (obj*[]) | SegmentDescriptor objects |
| 0x10 | 4 | segmentCount | Written early, right after the version. Must be > 0. |
| 0x14 | 4 | listBCount | |
| 0x18 | 4 | listB (obj*[]) | |
| 0x1C | 4 | listACount | |
| 0x20 | 4 | listA (obj*[]) | In hillside the first entry is a TrackDetailDescriptor. |
| 0x24 | 4 | extra (obj*) | Null in hillside. |
| 0x28 | 4 | ? | Not serialized. |
| 0x2C | 4 | unknown2C | 4 raw bytes. hillside: `36 8D 27 40` (2.618 as a float). |
| 0x30 | 1 | night | CenterLine `night`. Confirmed: 0 on day tracks, 1 on bristol_night and others. |
| 0x31 | 8×3 | lighting[3] (double) | CenterLine `unk_1..unk_3`. Day tracks use 0.2; bristol_night uses 0.5. |
| 0x49 | 4 | recordCount | |
| 0x4D | 4 | records | 48-byte records. |

## Transfer order (`0048BAA0`, the same code reads and writes)

```
Descriptor base                 int32 1, string name
int32 version                   8 when writing; the reader rejects anything above 8
int32 segmentCount              +0x10
if v >= 3:   uint8  night       +0x30
if v >= 8:   double lighting[0..2]   +0x31 +0x39 +0x41
             uint32 unknown2C        +0x2C
elif v >= 3: read and discard 5 doubles (v3-4), 13 (v5), or 21 (v6-7)
if v >= 7:   int32 recordCount  +0x49
             if > 0: ptrButNotData(count*48), then count*48 bytes   +0x4D
if v == 2 or v >= 4:
             object extra       +0x24
             int32 countA; if > 0: ptrButNotData(countA*4), then countA objects   +0x1C/+0x20
int32 countB; if > 0: ptrButNotData(countB*4), then countB objects                +0x14/+0x18
ptrButNotData(segmentCount*4), then segmentCount objects                          +0x0C   (fails if segmentCount <= 0)
```

Defaults applied when reading, before any fields: `night=0`, `lighting={0.2,0.2,0.2}`, `unknown2C=0`. For versions below 7: `recordCount=0` and `records=null`.

## hillside.ptf, annotated

```
0x00 01000000  0x04 02000000  0x08 10000000 "TrackDescriptor\0"   object 1, class 2
0x1C 01000000  0x20 00000000                                       base version 1, name null
0x24 08000000                                                      version 8
0x28 17000000                                                      23 segments
0x2C 00                                                            night = 0
0x2D 3 x 000000A09999C93F                                          lighting 0.2 0.2 0.2
0x45 368D2740                                                      unknown2C
0x49 01000000  0x4D 30000000  0x51 [48 bytes]                      1 record
0x81 00000000                                                      extra = null
0x85 06000000  0x89 18000000                                       list A: 6 objects
0x8D 03000000 04000000 16000000 "TrackDetailDescriptor\0"          first child
```

## The 48-byte record

hillside has 1, atlanta 5, martinsville 4, talladega 3 and watkins_glen 2.

In hillside it holds 16 bytes that look like a GUID (`83 26 91 1F 60 83 B5 01 9F F1 56 A7 1F 14 4A 81`), followed by the floats `0.2 2 7.5 7 0 0 0 0`. Its meaning is unknown.

## Verification

`ptfcheck` re-encodes the decoded header and compares it byte for byte with the original. It matches for hillside, atlanta, martinsville, talladega, watkins_glen, bristol_night, "Auto Club Night 2014" and Bristol_2019_BBMC_Night.

## Open questions

- What list A and list B hold. The class order in the file suggests list A is textures/appearances and list B is TSOs.
- The meaning of `unknown2C`, the 48-byte record, and `+0x28`.
- What vtable slot 6 (`0048C320`) does.
