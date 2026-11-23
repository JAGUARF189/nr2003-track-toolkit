"""Tiny static-analysis helper for 32-bit Papyrus binaries (capstone + pefile).

Usage from other scripts:
    from pe import Bin
    b = Bin(r"C:\\Program Files (x86)\\sandbox\\sandbox.exe")
    b.xrefs_to(va); b.dis(va, max_insns=200)
"""
import re
import struct
import sys

import capstone
import pefile


class Bin:
    def __init__(self, path):
        self.path = path
        self.pe = pefile.PE(path)
        self.base = self.pe.OPTIONAL_HEADER.ImageBase
        self.data = open(path, "rb").read()
        self.md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
        self.md.detail = True
        self.imports = {}  # IAT slot VA -> "dll!name"
        for entry in getattr(self.pe, "DIRECTORY_ENTRY_IMPORT", []):
            dll = entry.dll.decode(errors="replace").lower()
            for imp in entry.imports:
                name = imp.name.decode(errors="replace") if imp.name else f"ord{imp.ordinal}"
                self.imports[imp.address] = f"{dll}!{name}"
        self.text = next(s for s in self.pe.sections if s.Characteristics & 0x20000000)

    # --- address mapping -------------------------------------------------
    def off2va(self, off):
        return self.base + self.pe.get_rva_from_offset(off)

    def va2off(self, va):
        return self.pe.get_offset_from_rva(va - self.base)

    def read(self, va, n):
        o = self.va2off(va)
        return self.data[o:o + n]

    def u32(self, va):
        return struct.unpack("<I", self.read(va, 4))[0]

    def cstr(self, va, maxlen=200):
        try:
            raw = self.read(va, maxlen)
        except Exception:
            return None
        end = raw.find(b"\0")
        if end <= 0:
            return None
        s = raw[:end]
        if all(32 <= c < 127 or c in (9, 10, 13) for c in s):
            return s.decode()
        return None

    # --- searching -------------------------------------------------------
    def find_string(self, s):
        return [self.off2va(m.start()) for m in re.finditer(re.escape(s.encode()), self.data)]

    def xrefs_to(self, va):
        """VAs of instructions in .text that embed `va` as a 32-bit immediate/displacement."""
        needle = struct.pack("<I", va)
        t0 = self.text.PointerToRawData
        t1 = t0 + self.text.SizeOfRawData
        out = []
        for m in re.finditer(re.escape(needle), self.data[t0:t1]):
            pos = t0 + m.start()
            # back up a few bytes to find an instruction that decodes onto this operand
            for back in range(1, 8):
                start = pos - back
                insns = list(self.md.disasm(self.data[start:start + 16], self.off2va(start), 1))
                if insns and insns[0].size >= back + 4:
                    out.append(insns[0].address)
                    break
        return out

    def data_refs_to(self, va):
        """Offsets anywhere in the file (e.g. vtables, class tables) holding `va`."""
        needle = struct.pack("<I", va)
        return [self.off2va(m.start()) for m in re.finditer(re.escape(needle), self.data)]

    # --- disassembly -----------------------------------------------------
    def annotate(self, ins):
        notes = []
        for op in ins.operands:
            v = None
            if op.type == capstone.x86.X86_OP_IMM:
                v = op.imm & 0xFFFFFFFF
            elif op.type == capstone.x86.X86_OP_MEM and op.mem.base == 0 and op.mem.index == 0:
                v = op.mem.disp & 0xFFFFFFFF
            if v is None:
                continue
            if v in self.imports:
                notes.append(self.imports[v])
            elif ins.mnemonic in ("call", "jmp") and op.type == capstone.x86.X86_OP_IMM:
                t = self.thunk_target(v)
                if t:
                    notes.append(t)
            else:
                s = self.cstr(v)
                if s and len(s) >= 3:
                    notes.append(repr(s))
                elif op.type == capstone.x86.X86_OP_MEM:
                    try:
                        tgt = self.u32(v)
                        if tgt in self.imports:
                            notes.append(self.imports[tgt])
                    except Exception:
                        pass
        return notes

    def thunk_target(self, va):
        """Import name if `va` is a `jmp dword ptr [IAT]` thunk."""
        try:
            code = self.read(va, 6)
        except Exception:
            return None
        if code[:2] == b"\xff\x25":
            slot = struct.unpack("<I", code[2:6])[0]
            return self.imports.get(slot)
        return None

    @staticmethod
    def short(name):
        """'rts.dll!?transferBytes@Archive@@...' -> 'Archive::transferBytes'"""
        if "!" not in name:
            return name
        sym = name.split("!", 1)[1]
        if sym.startswith("?") and "@" in sym:
            parts = sym[1:].split("@@")[0].split("@")
            if len(parts) >= 2:
                return f"{parts[1]}::{parts[0]}"
        return sym

    def dis(self, va, max_insns=400, stop_at_ret=True, out=sys.stdout):
        code = self.read(va, max_insns * 15)
        n = 0
        for ins in self.md.disasm(code, va):
            notes = self.annotate(ins)
            line = f"{ins.address:08X}  {ins.mnemonic:6} {ins.op_str}"
            if notes:
                line = f"{line:60} ; {' | '.join(notes)}"
            print(line, file=out)
            n += 1
            if n >= max_insns or (stop_at_ret and ins.mnemonic in ("ret", "retn")):
                break

    def insns(self, va, max_insns=400, stop_at_ret=True):
        code = self.read(va, max_insns * 15)
        res = []
        for ins in self.md.disasm(code, va):
            res.append(ins)
            if len(res) >= max_insns or (stop_at_ret and ins.mnemonic in ("ret", "retn")):
                break
        return res

    def function_start(self, va, limit=0x4000):
        """Guess a function start by scanning back for int3/nop padding or a classic prologue."""
        for a in range(va, va - limit, -1):
            b = self.read(a, 3)
            if b[:3] == b"\x55\x8b\xec" and self.read(a - 1, 1) in (b"\xcc", b"\x90", b"\xc3"):
                return a
        return None


if __name__ == "__main__":
    b = Bin(sys.argv[1])
    print(f"base {b.base:#x}, imports {len(b.imports)}")
