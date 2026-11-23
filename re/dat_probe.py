"""Parse an NR2003 .dat index and compare entries against an mklib -u unpack folder."""
import os
import struct
import sys


def parse(path):
    d = open(path, "rb").read()
    magic, a = struct.unpack_from("<II", d, 0)
    pos = 10
    entries = []
    first_data = len(d)
    while pos < first_data:
        x0, flags = d[pos], d[pos + 1]
        size, stored, off = struct.unpack_from("<III", d, pos + 2)
        nlen = d[pos + 14]
        name = d[pos + 15:pos + 15 + nlen].decode("latin-1")
        term = d[pos + 15 + nlen]
        entries.append((pos, x0, flags, size, stored, off, name, term))
        first_data = min(first_data, off)
        pos += 15 + nlen + 1
    return d, magic, a, pos, entries


if __name__ == "__main__":
    d, magic, a, end, entries = parse(sys.argv[1])
    print(f"magic {magic:#x} field4 {a:#x} index ends {end:#x}, {len(entries)} entries, file {len(d)} bytes")
    from collections import Counter
    print("byte0 values:", Counter(e[1] for e in entries), "flags:", Counter(e[2] for e in entries),
          "terminators:", Counter(e[7] for e in entries))
    last = max(entries, key=lambda e: e[5])
    print(f"last entry ends at {last[5] + last[4]:#x} (file size {len(d):#x})")
    contiguous = all(entries[i][5] + entries[i][4] == entries[i + 1][5] for i in range(len(entries) - 1))
    print("entries contiguous in index order:", contiguous)
    if len(sys.argv) > 2:
        root = sys.argv[2]
        bad = 0
        for e in entries:
            p = os.path.join(root, e[6])
            if not os.path.exists(p) or os.path.getsize(p) != e[3]:
                bad += 1
                print("mismatch", e[6], e[3], os.path.getsize(p) if os.path.exists(p) else None)
            elif e[2] == 0 and open(p, "rb").read() != d[e[5]:e[5] + e[4]]:
                bad += 1
                print("raw content differs", e[6])
        print(f"{len(entries) - bad}/{len(entries)} entries match the unpacked files by size (raw ones by content)")
    for e in entries[:6]:
        print(f"  {e[0]:#06x} b0={e[1]} flags={e[2]} size={e[3]} stored={e[4]} off={e[5]:#x} {e[6]}")

