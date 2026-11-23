"""Survey NR2003 .mip files: header fields and chunk layout."""
import collections
import glob
import os
import struct
import sys


def hexs(b):
    return " ".join(f"{x:02X}" for x in b)


def chunks(data, start):
    """Yield (offset, tag, size_field, ...) assuming 4-char tag + u32 size records."""
    pos = start
    out = []
    while pos + 8 <= len(data) and len(out) < 40:
        tag = data[pos:pos + 4]
        if not all(65 <= c <= 90 for c in tag):
            break
        size = struct.unpack_from("<I", data, pos + 4)[0]
        out.append((pos, tag[::-1].decode(), size))
        pos += 8
    return out


def main(paths):
    by_type = collections.Counter()
    for p in paths:
        d = open(p, "rb").read()
        if len(d) < 32:
            continue
        typ, mapping = d[2], d[3]
        w, h = struct.unpack_from("<II", d, 4)
        by_type[typ] += 1
        print(f"{os.path.basename(p):28} {len(d):8}  type={typ:2} map={mapping} {1<<w}x{1<<h}  "
              f"hdr: {hexs(d[:32])}")
        print(f"{'':28} {'':8}  next: {hexs(d[32:96])}")
    print(by_type)


if __name__ == "__main__":
    files = []
    for a in sys.argv[1:]:
        files += glob.glob(a)
    main(files)
