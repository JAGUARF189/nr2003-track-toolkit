"""Condensed control flow of a transfer() routine: Archive calls (with the member they target),
compares, branches and internal calls. usage: python flow.py <va_hex> [max_insns]"""
import re
import sys
from pe import Bin

b = Bin(r"C:\Program Files (x86)\sandbox\sandbox.exe")
va = int(sys.argv[1], 16)
n = int(sys.argv[2]) if len(sys.argv) > 2 else 1500
ins = b.insns(va, n, stop_at_ret=False)
# stop at first int3/nop padding run after a ret
end = len(ins)
for k in range(1, len(ins)):
    if ins[k - 1].mnemonic.startswith("ret") and ins[k].mnemonic in ("nop", "int3"):
        end = k
        break
ins = ins[:end]
targets = {i.operands[0].imm for i in ins if i.mnemonic.startswith("j") and i.operands and i.operands[0].type == 2}
leas = {}
pend = []
for i in ins:
    lab = "L" if i.address in targets else " "
    m, op = i.mnemonic, i.op_str
    if m == "lea":
        reg = op.split(",")[0]
        leas[reg] = op.split(",", 1)[1].strip()
    if m == "push":
        pend.append(leas.get(op, op))
        pend = pend[-3:]
    if m == "call":
        notes = b.annotate(i)
        name = Bin.short(notes[0]) if notes else op
        name = name.replace("Archive::", "")
        if name in ("transferBytes", "transferPointerButNotData", "transferPointer"):
            print(f"{lab}{i.address:06X}   {name}({pend[-1] if pend else '?'}, {pend[-2] if len(pend)>1 else '?'})")
        elif name in ("transferString", "transferPersistentObject"):
            print(f"{lab}{i.address:06X}   {name}({pend[-1] if pend else '?'})")
        else:
            print(f"{lab}{i.address:06X}   call {name}")
        pend = []
    elif m in ("cmp", "test") or m.startswith("j") or m.startswith("ret") or m.startswith("fcomp") or m == "imul" or m == "shl" or (m == "mov" and ("[esi + 5]" in op or "[edi + 5]" in op or "+ 5]" in op)):
        print(f"{lab}{i.address:06X}     {m} {op}")
    elif lab == "L":
        print(f"L{i.address:06X}")
