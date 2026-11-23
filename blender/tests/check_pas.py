"""Sanity-check any .pas: right-handed transforms, outward winding, normals,
texture coordinates, material usage.  usage: python check_pas.py file.pas"""
import math
import re
import sys

text = open(sys.argv[1], encoding="ascii").read()
problems = 0


def bad(msg):
    global problems
    problems += 1
    print("FAIL", msg)


print(re.search(r"VERSION\t(\d+)", text).group(0).replace("\t", " "), "|",
      re.search(r"UNIT_CONVERSION\t(\S+)", text).group(0).replace("\t", " "))
for block in text.split("SHAPE {")[1:]:
    name = re.search(r'NAME "([^"]+)"', block).group(1)
    rows = [tuple(map(float, re.search(rf"XFORM_ROW{i} (\S+)\t(\S+)\t(\S+)", block).groups())) for i in range(3)]
    det = (rows[0][0] * (rows[1][1] * rows[2][2] - rows[1][2] * rows[2][1])
           - rows[0][1] * (rows[1][0] * rows[2][2] - rows[1][2] * rows[2][0])
           + rows[0][2] * (rows[1][0] * rows[2][1] - rows[1][1] * rows[2][0]))
    w = lambda p: tuple(p[0] * rows[0][k] + p[1] * rows[1][k] + p[2] * rows[2][k] for k in range(3))
    verts = [tuple(map(float, g)) for g in re.findall(r"VERTEX\s+\d+:\t(\S+)\t(\S+)\t(\S+)", block)]
    faces = [tuple(map(int, g)) for g in re.findall(r"FACE \d+:\s+(\d+)\s+(\d+)\s+(\d+)", block)]
    vn = {int(i): (float(x), float(y), float(z)) for i, x, y, z in re.findall(r"VERTEX_NORMAL (\d+)\t(\S+)\t(\S+)\t(\S+)", block)}
    inverted = 0
    for a, b, c in faces:
        pa, pb, pc = (w(verts[i]) for i in (a, b, c))
        e1 = [pb[k] - pa[k] for k in range(3)]
        e2 = [pc[k] - pa[k] for k in range(3)]
        cr = (e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0])
        avg = [sum(w(vn[i])[k] for i in (a, b, c)) for k in range(3)]
        if sum(cr[k] * avg[k] for k in range(3)) <= 0:
            inverted += 1
    lengths = [math.sqrt(sum(x * x for x in n)) for n in vn.values()]
    tex = re.findall(r"TEXTURE_FACE\s+\d+ BASE_TEXTURE (.*)", block)
    uvs = [float(x) for line in tex for x in re.findall(r"-?\d+\.\d+", line)]
    mats = re.findall(r'MATERIAL \{\s+NAME "([^"]+)"', block)
    textures = re.findall(r'BASE_TEXTURE "([^"]+)"', block)
    print(f"{name}: {len(verts)} verts, {len(faces)} faces, det {det:+.3f}, materials {mats}, textures {textures}, "
          f"textured faces {len(tex)}, UV range {min(uvs) if uvs else '-'}..{max(uvs) if uvs else '-'}")
    if det <= 0:
        bad(f"{name}: left-handed transform (make3do rejects it)")
    if inverted:
        bad(f"{name}: {inverted} faces wound against their normals")
    if lengths and (min(lengths) < 0.999 or max(lengths) > 1.001):
        bad(f"{name}: normals not unit length")
    if any(not math.isfinite(v) for p in verts for v in p):
        bad(f"{name}: non-finite vertex")
print("ALL GOOD" if not problems else f"{problems} PROBLEM(S)")
sys.exit(1 if problems else 0)
