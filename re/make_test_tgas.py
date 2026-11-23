"""Write known-content 256x256 TGAs for checking the mip decoder against makemip."""
import os
import struct
import sys

W = H = 256
out = sys.argv[1]
os.makedirs(out, exist_ok=True)


def write_tga(path, pixel):
    hdr = bytearray(18)
    hdr[2] = 2
    struct.pack_into("<HH", hdr, 12, W, H)
    hdr[16] = 32
    hdr[17] = 0x28  # top-left origin, 8 alpha bits
    body = bytearray()
    for y in range(H):
        for x in range(W):
            r, g, b, a = pixel(x, y)
            body += bytes((b, g, r, a))
    open(path, "wb").write(bytes(hdr) + body)


def base(x, y):
    return x, y, (x * 3 + y * 5) & 255


write_tga(os.path.join(out, "opaque.tga"), lambda x, y: (*base(x, y), 255))
write_tga(os.path.join(out, "alpha1.tga"), lambda x, y: (*base(x, y), 255 if ((x // 32 + y // 32) % 2) else 0))
write_tga(os.path.join(out, "alpha8.tga"), lambda x, y: (*base(x, y), x))
print("ok")
