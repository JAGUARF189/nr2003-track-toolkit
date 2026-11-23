import json, math, sys
d = json.load(open(sys.argv[1]))
N = {o["id"]: {"cls": o["class"], **{f[0]: f[2] for f in o["fields"]}} for o in d["objects"]}
root = N[d["root"]]
segs = [N[root[f"seg[{k}]"]] for k in range(root["segment_count"])]
def seglen(s):
    dx, dy = s["ex"] - s["sx"], s["ey"] - s["sy"]
    ch = math.hypot(dx, dy)
    if s["type"] == 0: return ch, 0, 0
    a = (s["eh"] - s["sh"] + math.pi) % (2 * math.pi) - math.pi
    r = ch / (2 * math.sin(abs(a) / 2))
    return r * abs(a), a, r
tot = 0
for k, s in enumerate(segs[:40]):
    L, a, r = seglen(s); tot += L
    xs = sorted([N[s[f"x[{i}]"]] for i in range(s["x_count"])], key=lambda x: x["dlat0"])
    fs = sorted([N[s[f"f[{i}]"]] for i in range(s["f_count"])], key=lambda x: x["dlat0"])
    ws = [N[s[f"w[{i}]"]] for i in range(s["w_count"])]
    # slope as longitudinal grade?  compare (alt1-alt0)/L with slope0/slope1 at each X
    grade = [(round((x["alt1"] - x["alt0"]) / max(L,1e-9), 4), round(x["slope0"], 4), round(x["slope1"], 4)) for x in xs[:3]]
    print(f"seg{k:2} t{s['type']} L={L:7.1f} ang={math.degrees(a):6.1f} r={r:6.1f} "
          f"X={[(round(x['dlat0'],1), round(x['alt0'],2)) for x in xs]} ")
    print(f"      grade/slope0/slope1 {grade}")
    print(f"      F={[(round(f['dlat0'],1), f['i15'], f['i19']) for f in fs]}  W={[(round(w['dlat0'],1), w['wall_type'], round(w['height0'],2)) for w in ws]}")
print("total length m", round(tot, 1), "ft", round(tot / 0.3048))
