"""Prototype .ptf reader, written from the sandbox.exe transfer() routines (see docs/ptf/).
Checked read-for-read against ptf_emu.py (Sandbox's own code in an emulator) before
being ported to C++.

usage: python ptf_read.py <file.ptf> [--check]     --check compares against ptf_emu.py
"""
import struct
import sys


class Archive:
    def __init__(self, data):
        self.d = data
        self.pos = 0
        self.objects = {}
        self.classes = {}
        self.reads = []          # (offset, size) per primitive, same granularity as ptf_emu
        self.versions = {}       # class -> set of versions seen

    def raw(self, n):
        if self.pos + n > len(self.d):
            raise EOFError(f"read past end at {self.pos:#x}")
        b = self.d[self.pos:self.pos + n]
        self.reads.append((self.pos, n))
        self.pos += n
        return b

    def i32(self): return struct.unpack("<i", self.raw(4))[0]
    def u8(self): return self.raw(1)[0]
    def f32(self): return struct.unpack("<f", self.raw(4))[0]
    def f64(self): return struct.unpack("<d", self.raw(8))[0]

    def string(self):
        start = self.pos
        n = struct.unpack("<i", self.d[start:start + 4])[0]
        self.pos += 4 + max(n, 0)
        self.reads.append((start, self.pos - start))
        return None if n == 0 else self.d[start + 4:start + 4 + n].rstrip(b"\0").decode("latin-1")

    def pointer(self):
        """transferPointer: int32 byte size + bytes (0 = null)."""
        start = self.pos
        n = struct.unpack("<i", self.d[start:start + 4])[0]
        self.pos += 4 + max(n, 0)
        self.reads.append((start, self.pos - start))
        return self.d[start + 4:start + 4 + n] if n else None

    def array_size(self):
        return self.i32()

    def obj(self):
        start = self.pos
        oid = struct.unpack("<i", self.d[start:start + 4])[0]
        p = start + 4
        if oid == 0:
            self.pos = p
            self.reads.append((start, 4))
            return None
        if oid in self.objects:
            self.pos = p
            self.reads.append((start, 4))
            return self.objects[oid]
        cid = struct.unpack("<i", self.d[p:p + 4])[0]
        p += 4
        if cid not in self.classes:
            ln = struct.unpack("<i", self.d[p:p + 4])[0]
            self.classes[cid] = self.d[p + 4:p + 4 + ln].rstrip(b"\0").decode()
            p += 4 + ln
        self.pos = p
        self.reads.append((start, p - start))
        cls = self.classes[cid]
        o = {"_class": cls, "_id": oid}
        self.objects[oid] = o          # registered before transfer, like rts.dll
        READERS[cls](self, o)
        return o

    def obj_list(self, count):
        if count <= 0:
            return []
        self.array_size()             # transferPointerButNotData(count*4)
        return [self.obj() for _ in range(count)]


def base(ar, o, cls, max_version):
    if ar.i32() != 1:
        raise ValueError(f"{cls}: Descriptor base version != 1 at {ar.pos:#x}")
    o["name"] = ar.string()
    v = ar.i32()
    ar.versions.setdefault(cls, set()).add(v)
    if v > max_version:
        raise ValueError(f"{cls}: version {v} > {max_version}")
    return v


def need(cond, cls, v):
    if not cond:
        raise NotImplementedError(f"{cls} version {v} is not supported yet")


# ---------------------------------------------------------------- classes
def r_track(ar, o):
    v = base(ar, o, "TrackDescriptor", 8)
    need(v == 8, "TrackDescriptor", v)
    o["segment_count"] = ar.i32()
    o["night"] = ar.u8()
    o["lighting"] = [ar.f64(), ar.f64(), ar.f64()]
    o["unknown2C"] = ar.raw(4).hex()
    n = ar.i32()
    o["records"] = []
    if n > 0:
        ar.array_size()
        blk = ar.raw(n * 48)               # one transferBytes(count*48)
        o["records"] = [blk[i:i + 48].hex() for i in range(0, n * 48, 48)]
    o["extra"] = ar.obj()
    o["details"] = ar.obj_list(ar.i32())
    o["tso_refs"] = ar.obj_list(ar.i32())
    if o["segment_count"] <= 0:
        raise ValueError("no segments")
    ar.array_size()
    o["segments"] = [ar.obj() for _ in range(o["segment_count"])]


def r_detail(ar, o):
    v = base(ar, o, "TrackDetailDescriptor", 0)
    o["appearance"] = ar.obj()
    o["d10"], o["d18"], o["d28"], o["d20"], o["d30"] = (ar.f64() for _ in range(5))
    o["p38"] = ar.pointer()
    o["p3c"] = ar.pointer()


def r_appearance(ar, o):
    v = base(ar, o, "AppearanceDescriptor", 2)
    need(v == 2, "AppearanceDescriptor", v)
    o["textures"] = [ar.obj() for _ in range(7)]   # +0x0c..+0x24
    o["v28"] = [struct.unpack("<3d", ar.raw(24))]
    o["v40"] = [struct.unpack("<3d", ar.raw(24))]
    o["v58"] = [struct.unpack("<3d", ar.raw(24))]
    o["d70"], o["d78"], o["d80"] = ar.f64(), ar.f64(), ar.f64()
    o["i88"] = ar.i32()


def r_texture(ar, o):
    v = base(ar, o, "TextureDescriptor", 2)
    need(v == 2, "TextureDescriptor", v)
    o["file"] = ar.string()
    o["flag"] = ar.u8()


def r_tso(ar, o):
    v = base(ar, o, "TSODescriptor", 3)
    need(v == 3, "TSODescriptor", v)
    o["file"] = ar.string()
    o["s10"] = ar.string()
    o["flag"] = ar.u8()


def r_tsoref(ar, o):
    v = base(ar, o, "TSOReferenceDescriptor", 6)
    need(v == 6, "TSOReferenceDescriptor", v)
    o["x"], o["y"], o["z"], o["rot1"], o["rot2"], o["rot3"] = (ar.f64() for _ in range(6))
    o["tso"] = ar.obj()
    o["i40"] = ar.i32()
    no_extra = ar.u8()
    o["extra"] = None
    if not no_extra:
        o["extra"] = (ar.raw(32).hex(), ar.i32(), ar.i32(), ar.i32())
    n = ar.i32()
    o["items"] = ar.raw(n * 40).hex() if n > 0 else ""
    o["sound"] = ar.string()


def r_segment(ar, o):
    v = base(ar, o, "SegmentDescriptor", 2)
    need(v == 2, "SegmentDescriptor", v)
    lists = []
    for _ in range(3):
        n = ar.i32()
        lists.append(ar.obj_list(n))
    o["x_sections"], o["f_sections"], o["w_sections"] = lists
    o["type"] = ar.i32()
    o["sx"], o["sy"], o["sh"], o["ex"], o["ey"], o["eh"] = (ar.f64() for _ in range(6))
    o["b40"], o["b41"] = ar.u8(), ar.u8()
    n = ar.i32()
    o["rec12"] = ar.raw(n * 12).hex() if n > 0 and ar.array_size() is not None else ""
    n = ar.i32()
    o["rec16"] = ar.raw(n * 16).hex() if n > 0 and ar.array_size() is not None else ""


def r_xsec(ar, o):
    v = base(ar, o, "X_SectionDescriptor", 3)
    need(v == 3, "X_SectionDescriptor", v)
    o["dlat0"], o["alt0"], o["slope0"] = ar.f32(), ar.f64(), ar.f32()
    o["dlat1"], o["alt1"], o["slope1"] = ar.f32(), ar.f64(), ar.f32()
    o["b14"] = ar.u8()
    # Two optional groups, each led by a mode byte: != 0 adds (f32, i32); == 2 also adds (u8, u8)
    for key in ("a", "b"):
        mode = ar.u8()
        g = {"mode": mode}
        if mode != 0:
            g["f"], g["i"] = ar.f32(), ar.i32()
        if mode == 2:
            g["u0"], g["u1"] = ar.u8(), ar.u8()
        o[key] = g


def layer_f(ar, out):
    shared = ar.u8()
    if shared:
        out.append({"shared": shared})
        return
    out.append({"shared": 0, "modes": [(ar.u8(), ar.f64()) for _ in range(2)],
                "d": [ar.f64() for _ in range(3)]})


def layer_w(ar, out):
    shared = ar.u8()
    if shared:
        out.append({"shared": shared})
        return
    out.append({"shared": 0, "modes": [(ar.u8(), ar.f64(), ar.f64()) for _ in range(2)]})


# Appearance texture slot used by layer k (F and W helpers): +0xc, +0x10, +0x18, +0x14, +0x1c, +0x20
LAYER_SLOTS = [0, 1, 3, 2, 4, 5]


def layers(ar, app, fn):
    out = []
    for k in range(6):
        if app["textures"][LAYER_SLOTS[k]] is not None:
            fn(ar, out)
        else:
            out.append(None)
    return out


def has_texture(app):
    t = app["textures"]
    return any(t[i] is not None for i in (0, 1, 2, 3, 5))   # +0xc +0x10 +0x14 +0x18 +0x20


def r_fsec(ar, o):
    v = base(ar, o, "F_SectionDescriptor", 5)
    need(v == 5, "F_SectionDescriptor", v)
    o["dlat0"], o["dlat1"] = ar.f32(), ar.f32()
    o["b14"] = ar.u8()
    o["i19"] = ar.i32()
    o["i15"] = ar.i32()
    o["appearance"] = app = ar.obj()
    if app is None or not has_texture(app):
        return
    o["coords"] = ar.obj()
    o["layers"] = layers(ar, app, layer_f)


def r_wsec(ar, o):
    v = base(ar, o, "W_SectionDescriptor", 7)
    need(v == 7, "W_SectionDescriptor", v)
    o["dlat0"], o["dlat1"] = ar.f32(), ar.f32()
    o["b14"] = ar.u8()
    o["i55"] = ar.i32()
    o["height0"], o["height1"], o["width0"], o["width1"], o["d35"], o["d3d"] = (ar.f64() for _ in range(6))
    o["i45"] = ar.i32()
    o["i49"] = ar.i32()
    if o["i49"] == 1:
        o["block40"] = ar.raw(40).hex()
    o["wall_type"] = ar.i32()
    n = ar.i32()
    o["subsections"] = subs = []
    for _ in range(n):
        s = {"d28": ar.f64(), "faces": []}
        for face in range(5):
            app = ar.obj()
            f = {"appearance": app}
            if app is not None and has_texture(app):
                f["coords"] = ar.obj()
                f["layers"] = layers(ar, app, layer_w)
            s["faces"].append(f)
        subs.append(s)


def r_coords(ar, o):
    v = base(ar, o, "TextureCoordsDescriptor", 1)
    need(v == 1, "TextureCoordsDescriptor", v)
    o["count"] = ar.i32()
    sub = ar.i32()
    need(sub == 3, "TextureCoordsDescriptor (sub)", sub)
    o["arrays"] = [ar.pointer() for _ in range(24)]


READERS = {
    "TrackDescriptor": r_track, "TrackDetailDescriptor": r_detail,
    "AppearanceDescriptor": r_appearance, "TextureDescriptor": r_texture,
    "TSODescriptor": r_tso, "TSOReferenceDescriptor": r_tsoref,
    "SegmentDescriptor": r_segment, "X_SectionDescriptor": r_xsec,
    "F_SectionDescriptor": r_fsec, "W_SectionDescriptor": r_wsec,
    "TextureCoordsDescriptor": r_coords,
}


def read(data):
    ar = Archive(data)
    root = ar.obj()
    return ar, root


def main():
    path = sys.argv[1]
    data = open(path, "rb").read()
    ar = Archive(data)
    err = None
    try:
        ar.obj()
    except Exception as e:  # noqa: BLE001
        err = e
    status = "OK" if err is None and ar.pos == len(data) else f"FAIL at {ar.pos:#x}: {err}"
    print(f"{status}  objects={len(ar.objects)}  {path}")
    if "--check" in sys.argv:
        from ptf_emu import PtfEmu
        emu = PtfEmu(data)
        try:
            emu.run()
        except Exception as e:  # noqa: BLE001
            print("  emulator stopped:", e)
        ref = [(t["off"], t["size"]) for t in emu.trace]
        for k, (a, b) in enumerate(zip(ar.reads, ref)):
            if a != b:
                print(f"  first difference at read #{k}: mine {a[0]:#x}+{a[1]}, Sandbox {b[0]:#x}+{b[1]}")
                break
        else:
            print(f"  reads identical to Sandbox ({len(ref)} reads)" if len(ref) == len(ar.reads)
                  else f"  length differs: mine {len(ar.reads)}, Sandbox {len(ref)}")
    return 0 if status == "OK" else 1


if __name__ == "__main__":
    sys.exit(main())
