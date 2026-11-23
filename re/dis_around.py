"""Disassemble a window of code that contains given addresses, finding an aligned start.
usage: python dis_around.py <exe> <addr_hex> [more addrs...] [--after N]"""
import sys

from pe import Bin

exe = sys.argv[1]
addrs = [int(a, 16) for a in sys.argv[2:] if not a.startswith("--")]
after = 0x60
if "--after" in sys.argv:
    after = int(sys.argv[sys.argv.index("--after") + 1], 16)
b = Bin(exe)
lo, hi = min(addrs), max(addrs)
for start in range(lo - 0x80, lo):
    ins = b.insns(start, 400, stop_at_ret=False)
    got = {i.address for i in ins}
    if all(a in got for a in addrs):
        for i in ins:
            if i.address > hi + after:
                break
            n = b.annotate(i)
            note = ("; " + " | ".join(n)) if n else ""
            print(f"{i.address:08X}  {i.mnemonic:6} {i.op_str:42} {note}")
        break
else:
    print("no aligned start found")
