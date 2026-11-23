"""Dump rts.dll's Archive primitives (the on-disk encoding of every .ptf field)."""
from pe import Bin

b = Bin(r"C:\Program Files (x86)\sandbox\rts.dll")
exports = {e.name.decode(): b.base + e.address for e in b.pe.DIRECTORY_ENTRY_EXPORT.symbols if e.name}
WANT = ["transferString", "transferPointerButNotData", "transferPointer@", "transferBytes",
        "transferPersistentObject", "OpenArchive", "CloseArchive"]

with open("out/rts_archive.asm", "w") as f:
    for name, va in sorted(exports.items(), key=lambda kv: kv[1]):
        if "@Archive@@" in name and any(w in name for w in WANT):
            print(f"=== {Bin.short('rts.dll!' + name)}  @ {va:08X}   ({name})", file=f)
            b.dis(va, 160, stop_at_ret=False, out=f)
            print(file=f)
    print("=== all rts.dll exports mentioning Archive/Persistent ===", file=f)
    for name, va in sorted(exports.items(), key=lambda kv: kv[1]):
        if "Archive" in name or "Persistent" in name:
            print(f"{va:08X} {name}", file=f)
print("written out/rts_archive.asm")
