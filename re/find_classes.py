"""Locate every *Descriptor class in sandbox.exe: registration, factory, ctor, vtable, transfer.
Method (HANDOFF): first code xref to the class-name string is the PersistentLoadingInfo
registration (name + factory); the factory allocates and calls the ctor, which stores the vtable;
vtable slot 0 is transfer(), slot 1 getClassName()."""
import sys
from pe import Bin

SANDBOX = r"C:\Program Files (x86)\sandbox\sandbox.exe"
NAMES = ["TrackDescriptor", "TrackDetailDescriptor", "AppearanceDescriptor", "TextureDescriptor",
         "TSOReferenceDescriptor", "TSODescriptor", "SegmentDescriptor", "X_SectionDescriptor",
         "F_SectionDescriptor", "W_SectionDescriptor", "TextureCoordsDescriptor"]
b = Bin(SANDBOX)

def exact_string(name):
    return [va for va in b.find_string(name + "\0") if b.read(va - 1, 1) in (b"\0",)]

def calls_and_imms(va, n=40):
    out = []
    for i in b.insns(va, n):
        out.append(i)
    return out

for name in NAMES:
    vas = exact_string(name)
    print(f"== {name}: string {[hex(v) for v in vas]}")
    for sva in vas:
        xs = b.xrefs_to(sva)
        print("   xrefs", [hex(x) for x in xs])
        if not xs:
            continue
        # registration: look at the instructions around the first xref for another push imm (factory)
        reg = xs[0]
        ins = b.insns(reg - 0x10, 20, stop_at_ret=False)
        fac = None
        for i in ins:
            if i.mnemonic in ("push", "mov") and i.address >= reg - 0x10 and i.address <= reg + 0x10:
                for op in i.operands:
                    if op.type == 2:  # imm
                        v = op.imm & 0xFFFFFFFF
                        if 0x401000 <= v < 0x4A0000 and v != sva:
                            fac = v
        print(f"   factory? {fac and hex(fac)}")
        if not fac:
            continue
        ctor = None; size = None
        for i in b.insns(fac, 30):
            if i.mnemonic == "push" and i.operands[0].type == 2 and size is None and (i.operands[0].imm < 0x1000):
                size = i.operands[0].imm
            if i.mnemonic == "call":
                t = i.operands[0].imm & 0xFFFFFFFF if i.operands[0].type == 2 else None
                if t and not b.thunk_target(t):
                    ctor = t
        print(f"   alloc size? {size and hex(size)}  ctor? {ctor and hex(ctor)}")
        if not ctor:
            continue
        vt = None
        for i in b.insns(ctor, 80):
            if i.mnemonic == "mov" and len(i.operands) == 2 and i.operands[0].type == 3 and i.operands[1].type == 2:
                v = i.operands[1].imm & 0xFFFFFFFF
                if 0x4A0000 <= v < 0x4C0000:
                    vt = v
        if vt:
            slots = [b.u32(vt + 4 * k) for k in range(8)]
            print(f"   vtable {vt:#x}: " + " ".join(f"{s:08X}" for s in slots))
