"""Summarise ptf_emu traces into per-class field layouts.
usage: python schema.py trace1.json [trace2.json ...]
For each class: the distinct field sequences seen (child objects collapsed to 'OBJ'),
how often, and sample values of every member offset decoded as int/float/double."""
import collections
import json
import struct
import sys

seqs = collections.defaultdict(collections.Counter)
samples = collections.defaultdict(lambda: collections.defaultdict(list))
for path in sys.argv[1:]:
    d = json.load(open(path))
    objs = {o["id"]: o for o in d["objects"]}
    per = collections.defaultdict(list)
    for t in d["trace"]:
        oid = t["obj"]
        if oid is None or oid not in objs:
            continue
        if t["kind"] == "objref":
            per[oid].append("OBJ")
            continue
        tok = f"{t['member']:#x}:{t['size']}" if t["kind"] not in ("local", "string") else (
            f"local:{t['size']}" if t["kind"] == "local" else f"str@{t['member']:#x}")
        if t["kind"] == "arraysize":
            tok = f"N@{t['member']:#x}"
        if t["kind"].startswith("array@"):
            tok = f"{t['kind']}[{t['size']}]"
        if per[oid] and per[oid][-1].startswith(tok.split("[")[0] + "[") and t["kind"].startswith("array@"):
            continue  # collapse repeated array element reads
        per[oid].append(tok)
        raw = bytes.fromhex(t["raw"])
        cls = objs[oid]["cls"]
        key = tok
        if len(samples[cls][key]) < 400:
            if len(raw) == 4:
                i = struct.unpack("<i", raw)[0]; f = struct.unpack("<f", raw)[0]
                samples[cls][key].append((i, f))
            elif len(raw) == 8:
                samples[cls][key].append((None, struct.unpack("<d", raw)[0]))
            elif len(raw) == 1:
                samples[cls][key].append((raw[0], None))
            elif t.get("str") is not None:
                samples[cls][key].append((t["str"], None))
    for oid, toks in per.items():
        # collapse runs of OBJ
        c = []
        for tk in toks:
            if tk == "OBJ" and c and c[-1].startswith("OBJ"):
                c[-1] = "OBJ*"
            else:
                c.append(tk)
        seqs[objs[oid]["cls"]][" ".join(c)] += 1

for cls in seqs:
    print(f"\n######## {cls}")
    for s, n in seqs[cls].most_common(8):
        print(f"  [{n:5}] {s}")
    print("  fields:")
    for key, vals in samples[cls].items():
        if not vals:
            continue
        a = vals[0]
        if isinstance(a[0], str):
            ex = sorted({str(v[0]) for v in vals})[:6]
            print(f"    {key:16} strings e.g. {ex}")
        elif a[0] is None:
            xs = [v[1] for v in vals]
            print(f"    {key:16} double min {min(xs):.6g} max {max(xs):.6g} e.g. {[round(x,4) for x in xs[:5]]}")
        elif a[1] is None:
            xs = collections.Counter(v[0] for v in vals)
            print(f"    {key:16} byte {dict(xs.most_common(6))}")
        else:
            ints = collections.Counter(v[0] for v in vals)
            fl = [v[1] for v in vals]
            if len(ints) <= 8 and all(abs(i) < 100000 for i in ints):
                print(f"    {key:16} int {dict(ints.most_common(8))}")
            else:
                fin = [x for x in fl if x == x and abs(x) < 1e7]
                print(f"    {key:16} float min {min(fin) if fin else 0:.6g} max {max(fin) if fin else 0:.6g} e.g. {[round(x,4) for x in fl[:5]]}")
