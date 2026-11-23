"""Decode a .ptf by running sandbox.exe's own transfer() routines in a CPU emulator.

Every rts.dll Archive call is replaced by a Python stand-in that reads from the real file,
and every byte read is attributed to (object, class, member offset). The result is the
exact field layout Sandbox uses, with no hand-reading of disassembly.

usage: python ptf_emu.py <file.ptf> [--trace out.txt] [--json out.json]
Requires: unicorn, pefile (pip --user).
"""
import json
import struct
import sys

import pefile
from unicorn import Uc, UcError, UC_ARCH_X86, UC_MODE_32, UC_HOOK_CODE, UC_HOOK_MEM_UNMAPPED
from unicorn.x86_const import (UC_X86_REG_EAX, UC_X86_REG_EBX, UC_X86_REG_ECX, UC_X86_REG_EDX,
                               UC_X86_REG_ESI, UC_X86_REG_EDI, UC_X86_REG_EBP, UC_X86_REG_ESP,
                               UC_X86_REG_EIP, UC_X86_REG_EFLAGS)

SANDBOX = r"C:\Program Files (x86)\sandbox\sandbox.exe"

# Found by re/find_classes.py: class name -> factory (no args, returns object in eax)
FACTORIES = {
    "TrackDescriptor": 0x48B230, "TrackDetailDescriptor": 0x48B550,
    "AppearanceDescriptor": 0x47F020, "TextureDescriptor": 0x47EFB0,
    "TSOReferenceDescriptor": 0x48B4E0, "TSODescriptor": 0x48B470,
    "SegmentDescriptor": 0x48B2A0, "X_SectionDescriptor": 0x48B310,
    "F_SectionDescriptor": 0x48B380, "W_SectionDescriptor": 0x48B400,
    "TextureCoordsDescriptor": 0x47EA00,
}

# Functions replaced by `mov eax,1; ret N` (address -> N bytes of stack args)
PATCH_RETURN_1 = {
    0x4893E0: 8,   # TextureDescriptor vtable slot 6: load the .mip image from disk
    0x47AD00: 0,   # load a .3do by name (cdecl name, out**), called by TSODescriptor::transfer
}

HEAP, HEAP_SIZE = 0x10000000, 0x08000000
STACK, STACK_SIZE = 0x7F000000, 0x00400000
STUBS, STUBS_SIZE = 0x60000000, 0x00010000
STOP = 0x6000FFF0          # return address for top-level / nested calls
REGS = [UC_X86_REG_EAX, UC_X86_REG_EBX, UC_X86_REG_ECX, UC_X86_REG_EDX, UC_X86_REG_ESI,
        UC_X86_REG_EDI, UC_X86_REG_EBP, UC_X86_REG_ESP, UC_X86_REG_EFLAGS]


class PtfEmu:
    def __init__(self, ptf_bytes):
        self.f = ptf_bytes
        self.pos = 0
        self.pe = pefile.PE(SANDBOX)
        base = self.pe.OPTIONAL_HEADER.ImageBase
        img = self.pe.get_memory_mapped_image()
        self.uc = uc = Uc(UC_ARCH_X86, UC_MODE_32)
        size = (len(img) + 0xFFF) & ~0xFFF
        uc.mem_map(base, size)
        uc.mem_write(base, img)
        uc.mem_map(HEAP, HEAP_SIZE)
        uc.mem_map(STACK, STACK_SIZE)
        uc.mem_map(STUBS, STUBS_SIZE)
        uc.mem_write(STOP, b"\xF4")  # never executed: emu_start(until=STOP)
        self.heap_top = HEAP + 0x1000
        self.archive = self.alloc(0x40)  # fake Archive; [archive+5] = 0 means reading

        # Point every IAT slot at its own stub. Python stubs are `nop; ret N` with a hook on the nop.
        self.stub_handlers = {}
        self.stub_names = {}
        next_stub = STUBS
        for entry in self.pe.DIRECTORY_ENTRY_IMPORT:
            dll = entry.dll.decode().lower()
            for imp in entry.imports:
                name = imp.name.decode() if imp.name else f"ord{imp.ordinal}"
                full = f"{dll}!{name}"
                stub = next_stub
                next_stub += 0x20
                uc.mem_write(imp.address, struct.pack("<I", stub))
                self.stub_names[stub] = full
                code, handler = self.make_stub(full)
                uc.mem_write(stub, code)
                if handler:
                    self.stub_handlers[stub] = handler
        # Asset loaders called during transfer() (they open .mip/.3do files): make them succeed.
        for va, ret_bytes in PATCH_RETURN_1.items():
            uc.mem_write(va, bytes.fromhex("B801000000") + (bytes([0xC2]) + struct.pack("<H", ret_bytes) if ret_bytes else bytes([0xC3])))
        uc.hook_add(UC_HOOK_CODE, self.on_stub, begin=STUBS, end=STUBS + STUBS_SIZE - 1)
        uc.hook_add(UC_HOOK_MEM_UNMAPPED, self.on_bad_mem)

        self.objects = {}      # file object id -> dict
        self.classes = {}      # class id -> name
        self.regions = []      # (start, end, owner_obj, member_off, kind)
        self.trace = []        # (file_off, size, obj_id, class, member_off, kind, raw)
        self.obj_stack = []

    # ---------------------------------------------------------------- memory helpers
    def alloc(self, n):
        p = self.heap_top
        self.heap_top = (self.heap_top + n + 15) & ~15
        self.uc.mem_write(p, b"\0" * n)
        return p

    def rd32(self, va):
        return struct.unpack("<I", self.uc.mem_read(va, 4))[0]

    def wr32(self, va, v):
        self.uc.mem_write(va, struct.pack("<I", v & 0xFFFFFFFF))

    def arg(self, i):  # i-th stack argument at a stub's entry (after return address)
        return self.rd32(self.uc.reg_read(UC_X86_REG_ESP) + 4 + 4 * i)

    def take(self, n):
        if self.pos + n > len(self.f):
            raise EOFError(f"read past end at {self.pos:#x} (+{n})")
        b = self.f[self.pos:self.pos + n]
        self.pos += n
        return b

    def owner_of(self, va):
        for start, end, obj, off0, kind in reversed(self.regions):
            if start <= va < end:
                return obj, va - start + off0, kind
        if STACK <= va < STACK + STACK_SIZE:
            return None, 0, "local"
        return None, va, "?"

    # ---------------------------------------------------------------- stubs
    def make_stub(self, full):
        short = full.split("!")[1]
        native = {
            # acos(x) = atan2(sqrt(1-x*x), x), x87, argument and result in st(0)
            "_CIacos": bytes.fromhex("D9C0 D8C8 D9E8 DEE1 D9FA D9C9 D9F3 C3".replace(" ", "")),
            # fmod: st(1)=x, st(0)=y -> st(0)=fmod(x,y)
            "_CIfmod": bytes.fromhex("D9C9 D9F8 DFE0 9E 7A F9 DDD9 C3".replace(" ", "")),
        }
        if short in native:
            return native[short], None
        table = {
            "?transferBytes@Archive@@QAE?AW4success@@PAXI@Z": (8, self.s_transferBytes),
            "?transferString@Archive@@QAE?AW4success@@PAPAD@Z": (4, self.s_transferString),
            "?transferPointerButNotData@Archive@@QAE?AW4success@@PAPAXI@Z": (8, self.s_transferPtrNoData),
            "?transferPointer@Archive@@QAE?AW4success@@PAPAXI@Z": (8, self.s_transferPointer),
            "?transferPersistentObject@Archive@@QAE?AW4success@@PAPAVPersistentObject@@@Z": (4, self.s_transferObject),
            "?_memAlloc@@YA?AW4success@@PAPAXK@Z": (0, self.s_memAlloc),
            "?_memFree@@YAXPAX@Z": (0, lambda: 0),
            "?release@PersistentObject@@SA_NPAV1@@Z": (0, lambda: 1),
            "_isnan": (0, self.s_isnan),
            "?fileGetFailedLoadCallback@@YA?AW4success@@PAP6A?AW4eFailedLoadReturnFlags@@H@Z@Z": (0, self.s_noCallback),
        }
        if short in table:
            n, h = table[short]
            ret = b"\xC3" if n == 0 else b"\xC2" + struct.pack("<H", n)
            return b"\x90" + ret, h
        return b"\x90\xC3", self.unknown_import(full)

    def unknown_import(self, full):
        def h():
            ra = self.rd32(self.uc.reg_read(UC_X86_REG_ESP))
            raise RuntimeError(f"unhandled import called: {full} (return address {ra:#x})")
        return h

    def on_stub(self, uc, address, size, user):
        h = self.stub_handlers.get(address)
        if h is None:
            return
        rv = h()
        if rv is not None:
            uc.reg_write(UC_X86_REG_EAX, rv & 0xFFFFFFFF)

    def on_bad_mem(self, uc, access, address, size, value, user):
        eip = uc.reg_read(UC_X86_REG_EIP)
        print(f"!! unmapped access {address:#x} at eip {eip:#x}", file=sys.stderr)
        return False

    # Archive::transferBytes(void* p, uint n)
    def s_transferBytes(self):
        p, n = self.arg(0), self.arg(1)
        off = self.pos
        data = self.take(n)
        self.uc.mem_write(p, data)
        obj, moff, kind = self.owner_of(p)
        top = self.obj_stack[-1] if self.obj_stack else None
        self.trace.append(dict(off=off, size=n, obj=obj if obj is not None else top, member=moff,
                               kind=kind, raw=data.hex(), depth=len(self.obj_stack)))
        return 1

    # Archive::transferString(char** pp): int32 len (incl NUL, 0 = null) + bytes
    def s_transferString(self):
        pp = self.arg(0)
        off = self.pos
        n = struct.unpack("<i", self.take(4))[0]
        s = None
        if n == 0:
            self.wr32(pp, 0)
        else:
            raw = self.take(n)
            p = self.alloc(n)
            self.uc.mem_write(p, raw)
            self.wr32(pp, p)
            s = raw.rstrip(b"\0").decode("latin-1")
        obj, moff, kind = self.owner_of(pp)
        self.trace.append(dict(off=off, size=self.pos - off, obj=obj, member=moff, kind="string",
                               raw=self.f[off:self.pos].hex(), str=s, depth=len(self.obj_stack)))
        return 1

    # Archive::transferPointerButNotData(void** pp, uint size): int32 size; allocate on read
    def s_transferPtrNoData(self):
        pp = self.arg(0)
        off = self.pos
        n = struct.unpack("<i", self.take(4))[0]
        obj, moff, kind = self.owner_of(pp)
        if n == 0:
            self.wr32(pp, 0)
        else:
            p = self.alloc(n)
            self.wr32(pp, p)
            self.regions.append((p, p + n, obj, 0, f"array@{moff:#x}"))
        self.trace.append(dict(off=off, size=4, obj=obj, member=moff, kind="arraysize",
                               raw=self.f[off:self.pos].hex(), depth=len(self.obj_stack)))
        return 1

    # Archive::transferPointer(void** pp, uint size): int32 size, then the bytes
    def s_transferPointer(self):
        pp = self.arg(0)
        off = self.pos
        n = struct.unpack("<i", self.take(4))[0]
        obj, moff, kind = self.owner_of(pp)
        if n == 0:
            self.wr32(pp, 0)
        else:
            p = self.alloc(n)
            self.uc.mem_write(p, self.take(n))
            self.wr32(pp, p)
        self.trace.append(dict(off=off, size=self.pos - off, obj=obj, member=moff, kind="pointer",
                               raw=self.f[off:self.pos].hex(), depth=len(self.obj_stack)))
        return 1

    def s_memAlloc(self):
        pp, n = self.arg(0), self.arg(1)
        p = self.alloc(n)
        self.wr32(pp, p)
        owner, moff, _ = self.owner_of(pp)
        if owner is None and self.obj_stack:
            owner = self.obj_stack[-1]
        if self.obj_stack:   # buffers allocated inside a transfer(), e.g. TSO extra data
            self.regions.append((p, p + n, owner, 0, f"heap{n}"))
        return 1

    def s_noCallback(self):
        self.wr32(self.arg(0), 0)
        return 1

    def s_isnan(self):
        lo, hi = self.arg(0), self.arg(1)
        d = struct.unpack("<d", struct.pack("<II", lo, hi))[0]
        return 1 if d != d else 0

    # Archive::transferPersistentObject(PersistentObject** pp)
    def s_transferObject(self):
        pp = self.arg(0)
        parent, pmoff, pkind = self.owner_of(pp)
        if parent is None and self.obj_stack:   # arrays are read through a stack temporary
            parent, pmoff = self.obj_stack[-1], -1
        start = self.pos
        oid = struct.unpack("<i", self.take(4))[0]
        if oid == 0:
            self.wr32(pp, 0)
            self.trace.append(dict(off=start, size=4, obj=parent, member=pmoff, kind="objref",
                                   ref=0, raw=self.f[start:self.pos].hex(), depth=len(self.obj_stack)))
            return 1
        if oid in self.objects:
            self.wr32(pp, self.objects[oid]["va"])
            self.trace.append(dict(off=start, size=4, obj=parent, member=pmoff, kind="objref",
                                   ref=oid, raw=self.f[start:self.pos].hex(), depth=len(self.obj_stack)))
            return 1
        cid = struct.unpack("<i", self.take(4))[0]
        if cid not in self.classes:
            ln = struct.unpack("<i", self.take(4))[0]
            self.classes[cid] = self.take(ln).rstrip(b"\0").decode()
        cname = self.classes[cid]
        if cname not in FACTORIES:
            raise RuntimeError(f"unknown class {cname!r} at {start:#x}")
        self.trace.append(dict(off=start, size=self.pos - start, obj=parent, member=pmoff, kind="objref",
                               ref=oid, cls=cname, raw=self.f[start:self.pos].hex(), depth=len(self.obj_stack)))
        va = self.call(FACTORIES[cname])
        vt = self.rd32(va)
        size = self.alloc_size_guess(cname)
        rec = dict(id=oid, cls=cname, va=va, start=start, parent=parent, parent_member=pmoff)
        self.objects[oid] = rec
        self.regions.append((va, va + size, oid, 0, "obj"))
        self.wr32(pp, va)
        self.obj_stack.append(oid)
        ok = self.call(self.rd32(vt), this=va, args=[self.archive])
        self.obj_stack.pop()
        rec["end"] = self.pos
        if not ok:
            raise RuntimeError(f"{cname} #{oid} transfer() failed; object began at {start:#x}, now at {self.pos:#x}")
        return 1

    SIZES = {"TrackDescriptor": 0x51, "TrackDetailDescriptor": 0x40, "AppearanceDescriptor": 0x9D,
             "TextureDescriptor": 0x16, "TSOReferenceDescriptor": 0x54, "TSODescriptor": 0x19,
             "SegmentDescriptor": 0x6A, "X_SectionDescriptor": 0x4B, "F_SectionDescriptor": 0x127,
             "W_SectionDescriptor": 0x61, "TextureCoordsDescriptor": 0x74}

    def alloc_size_guess(self, cname):
        return self.SIZES[cname]

    # ---------------------------------------------------------------- calling into the exe
    def call(self, fn, this=None, args=()):
        uc = self.uc
        saved = [uc.reg_read(r) for r in REGS]
        nested = bool(self.obj_stack) or getattr(self, "_running", False)
        esp = saved[7] - 0x100 if nested else STACK + STACK_SIZE - 0x1000
        for a in reversed(args):
            esp -= 4
            self.wr32(esp, a)
        esp -= 4
        self.wr32(esp, STOP)
        uc.reg_write(UC_X86_REG_ESP, esp)
        if this is not None:
            uc.reg_write(UC_X86_REG_ECX, this)
        prev = getattr(self, "_running", False)
        self._running = True
        try:
            uc.emu_start(fn, STOP)
        finally:
            self._running = prev
        rv = uc.reg_read(UC_X86_REG_EAX)
        for r, v in zip(REGS, saved):
            if r != UC_X86_REG_EAX:
                uc.reg_write(r, v)
        return rv

    def run(self):
        root_pp = self.alloc(4)
        self.regions.append((root_pp, root_pp + 4, None, 0, "root"))
        self.s_transfer_root(root_pp)
        return self

    def s_transfer_root(self, pp):
        # Emulate a top-level transferPersistentObject(&root) without a stub frame.
        esp = STACK + STACK_SIZE - 0x2000
        self.uc.reg_write(UC_X86_REG_ESP, esp)
        self.wr32(esp + 4, pp)
        self.s_transferObject()


def main():
    path = sys.argv[1]
    data = open(path, "rb").read()
    emu = PtfEmu(data)
    err = None
    try:
        emu.run()
    except (RuntimeError, EOFError, UcError) as e:
        err = e
    print(f"{path}: {len(data)} bytes, consumed {emu.pos}, objects {len(emu.objects)}")
    counts = {}
    for o in emu.objects.values():
        counts[o["cls"]] = counts.get(o["cls"], 0) + 1
    for k, v in counts.items():
        print(f"  {v:5} {k}")
    if err:
        print(f"STOPPED: {err}")
    if "--trace" in sys.argv:
        out = sys.argv[sys.argv.index("--trace") + 1]
        with open(out, "w") as fh:
            for t in emu.trace:
                o = emu.objects.get(t["obj"], {}) if t["obj"] else {}
                cls = o.get("cls", "-")
                extra = t.get("str") or t.get("cls") or (f"ref {t['ref']}" if "ref" in t else "")
                fh.write(f"{t['off']:08X} {'  ' * t['depth']}{cls}#{t['obj']} +{t['member']:#04x} "
                         f"{t['kind']:10} {t['size']:4} {t['raw'][:48]} {extra}\n")
    if "--json" in sys.argv:
        out = sys.argv[sys.argv.index("--json") + 1]
        json.dump(dict(objects=list(emu.objects.values()), trace=emu.trace), open(out, "w"))
    return 0 if (err is None and emu.pos == len(data)) else 1


if __name__ == "__main__":
    sys.exit(main())
