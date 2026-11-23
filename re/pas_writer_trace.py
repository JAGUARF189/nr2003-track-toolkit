"""Trace maxascii.dle's PAS writer: for each function that references the
format strings, list the string pushes / calls in code order."""
import capstone
from pe import Bin

b = Bin(r"C:\Program Files (x86)\sandbox\papyplugs\maxascii.dle")

# All printable strings in the .data area that look like PAS formats/keywords.
strings = {}
lo, hi = b.off2va(0x8030), b.off2va(0x85C8)
va = lo
while va < hi:
    s = b.cstr(va, 120)
    if s and len(s) >= 2:
        strings[va] = s
        va += len(s) + 1
    else:
        va += 1

funcs = {}
for sva, s in strings.items():
    for x in b.xrefs_to(sva):
        f = b.function_start(x) or x
        funcs.setdefault(f, set()).add(x)

for f in sorted(funcs):
    print(f"\n===== function {f:08X} =====")
    for ins in b.insns(f, 3000, stop_at_ret=False):
        if ins.address > f and ins.mnemonic == "ret":
            print(f"{ins.address:08X}  ret")
            break
        notes = b.annotate(ins)
        show = False
        txt = ""
        for op in ins.operands:
            if op.type == capstone.x86.X86_OP_IMM and (op.imm & 0xFFFFFFFF) in strings:
                txt = repr(strings[op.imm & 0xFFFFFFFF])
                show = True
        if ins.mnemonic == "call":
            show = True
            txt = txt or (" | ".join(notes) if notes else ins.op_str)
        elif ins.mnemonic in ("jmp", "je", "jne", "jl", "jge", "jle", "jg") and False:
            show = True
        if show:
            print(f"{ins.address:08X}  {ins.mnemonic:5} {txt}")
