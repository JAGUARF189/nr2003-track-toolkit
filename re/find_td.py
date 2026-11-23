"""Step 1: locate TrackDescriptor's class registration and code in sandbox.exe."""
import sys

from pe import Bin

SANDBOX = r"C:\Program Files (x86)\sandbox\sandbox.exe"
b = Bin(SANDBOX)

print("rts.dll imports used by sandbox.exe:")
for slot, name in sorted(b.imports.items()):
    if name.startswith("rts.dll!") and ("Archive" in name or "Persistent" in name):
        print(f"  {slot:08X} {name}")

for va in b.find_string("TrackDescriptor"):
    full = b.cstr(va)
    print(f"\nstring {va:08X} {full!r}")
    for x in b.xrefs_to(va):
        print(f"  code xref {x:08X}")
        start = b.function_start(x)
        print(f"  enclosing function ~{start:08X}" if start else "  (no prologue found)")
    for d in b.data_refs_to(va):
        print(f"  data ref at {d:08X}")
