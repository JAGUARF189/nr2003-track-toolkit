"""Walk the record tree of an NR2003 .mip: tag(4, reversed) u32 flags u32 size, data padded to 4."""
import struct
import sys

CONTAINERS = {"BMAP", "STMP"}


def walk(d, pos, end, depth, out):
    while pos + 12 <= end:
        tag = d[pos:pos + 4][::-1].decode("latin-1")
        flags, size = struct.unpack_from("<II", d, pos + 4)
        body = pos + 12
        info = ""
        if tag == "BMHD":
            typ = d[body]
            w, h, pitch = struct.unpack_from("<III", d, body + 1)
            info = f"type={typ} {w}x{h} pitch={pitch} last={d[body+13]}"
        elif tag not in CONTAINERS:
            info = " ".join(f"{x:02X}" for x in d[body:body + min(size, 16)])
        out.append(f"{'  '*depth}{pos:#08x} {tag} flags={flags:#x} size={size} {info}")
        if tag in CONTAINERS:
            walk(d, body, body + size, depth + 1, out)
        pos = body + ((size + 3) & ~3)
    if pos != end:
        out.append(f"{'  '*depth}!! stopped at {pos:#x}, expected end {end:#x}")


def main(p, limit=60):
    d = open(p, "rb").read()
    print(f"== {p} ({len(d)} bytes)")
    print("header:", " ".join(f"{x:02X}" for x in d[:32]))
    out = []
    walk(d, 32, len(d), 0, out)
    for line in out[:limit]:
        print(line)
    if len(out) > limit:
        print(f"... {len(out) - limit} more records")


if __name__ == "__main__":
    for a in sys.argv[1:]:
        main(a)

