# .ptf container: the rts.dll `Archive` format

A `.ptf` file is not MFC `CArchive`. It's written by Papyrus's own `Archive` class, exported from `rts.dll`. The file is a graph of `PersistentObject`s, and each class has one `transfer(Archive&)` routine that both reads and writes.

All values are little-endian. Sizes are 32-bit.

## Primitives

These were disassembled from rts.dll; see `re/out/rts_archive.asm`.

| rts.dll export | Address | On disk |
|---|---|---|
| `Archive::transferBytes(void*, uint)` | `10001E20` | The raw bytes. |
| `Archive::transferString(char**)` | `10001E60` | Calls `transferPointer(&s, strlen(s)+1)`: an int32 length that counts the NUL (0 = null pointer), then the bytes including the NUL. |
| `Archive::transferPointerButNotData(void**, uint size)` | `10001F50` | An int32 byte size only (0 if the pointer is null). The caller then transfers the data itself, usually with `transferBytes`. |
| `Archive::transferPersistentObject(PersistentObject**)` | `10002000` | See below. |

`[archive+5]` is the direction flag: non-zero means writing.

## Objects

```
int32 objectId                  0 = null pointer
-- if objectId was already written: nothing else (back-reference) --
int32 classId
-- only the first time this classId appears: --
int32 nameLength, char name[nameLength]        e.g. 10 00 00 00 "TrackDescriptor\0"
-- then the object's own transfer() --
```

- **Shared ID counter.** Object IDs and class IDs come from one counter, `[archive+0xB]`, which starts at 1 and counts up as things are written. In hillside: TrackDescriptor is object 1, class 2; TrackDetailDescriptor is object 3, class 4; AppearanceDescriptor is object 5, class 6; TextureDescriptor is object 7, class 8.
- **Registration order.** When reading, the object is registered under its ID *before* its transfer runs, so cyclic references resolve.
- **Class lookup.** Class names are matched through the `PersistentLoadingInfo` list. Each class registers its name and a factory with it at startup; sandbox.exe does this for TrackDescriptor at `0048B270`.

## Descriptor base

`sandbox.exe 00486BB0` is the common base of every `*Descriptor`:

```
int32 baseVersion               always 1; the reader rejects anything else
string name                     Descriptor name, null (00 00 00 00) in stock tracks
```

## The file

A `.ptf` is a single root object (object 1, a `TrackDescriptor`) with everything else reachable from it. See [TrackDescriptor.md](TrackDescriptor.md).
