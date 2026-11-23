"""Check pas_writer output against Papyrus make3do (no Blender needed).

usage: python test_pas_writer.py <work dir> <make3do.exe> <some .mip to use as a texture>
"""
import math
import os
import shutil
import struct
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "nr2003_pas"))
import pas_writer as pw  # noqa: E402

work, make3do, tex = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(work, exist_ok=True)
shutil.copy(tex, os.path.join(work, "testtex.mip"))


def cube(name, rows=None, pos=(0, 0, 0)):
    """Unit cube, flat shaded (hard edges), +Z/-Z faces textured, others plain."""
    P = [(x, y, z) for z in (0, 1) for y in (0, 1) for x in (0, 1)]
    quads = [  # (corners counter-clockwise seen from outside, normal, material)
        ((0, 2, 3, 1), (0, 0, -1), 0),
        ((4, 5, 7, 6), (0, 0, 1), 0),
        ((0, 1, 5, 4), (0, -1, 0), 1),
        ((2, 6, 7, 3), (0, 1, 0), 1),
        ((0, 4, 6, 2), (-1, 0, 0), 1),
        ((1, 3, 7, 5), (1, 0, 0), 1),
    ]
    quad_uv = ((0, 0), (1, 0), (1, 1), (0, 1))
    tris, normals, uvs, mats = [], [], [], []
    for (a, b, c, d), n, mi in quads:
        for t in ((0, 1, 2), (0, 2, 3)):
            q = (a, b, c, d)
            tris.append(tuple(q[i] for i in t))
            normals.append(tuple(n for _ in t))
            uvs.append(tuple(quad_uv[i] for i in t))
            mats.append(mi)
    m = pw.Mesh(
        name=name,
        positions=[tuple(float(v) for v in p) for p in P],
        tris=tris,
        corner_normals=normals,
        corner_uvs=uvs,
        tri_material=mats,
        materials=[
            pw.Material("Painted Sign", diffuse=(1, 1, 1), base_texture="testtex.mip"),
            pw.Material("plain grey", diffuse=(0.5, 0.5, 0.5)),
        ],
        pos=pos,
    )
    if rows:
        m.rows = rows
    return m


c90 = ((0.0, 1.0, 0.0), (-1.0, 0.0, 0.0), (0.0, 0.0, 1.0))  # 90 degrees about Z
meshes = [
    cube("Box"),
    cube("Box.mirrored", rows=((-1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0)), pos=(5, 0, 0)),
    cube("Box moved", rows=c90, pos=(10, 20, 30)),
]
pas = os.path.join(work, "scene.pas")
open(pas, "w", newline="\n").write(pw.build_pas(meshes, source="test"))
open(os.path.join(work, "scene.psg"), "w", newline="\n").write(pw.build_psg("scene.pas", meshes))

ok = True


def check(cond, msg):
    global ok
    print(("PASS " if cond else "FAIL ") + msg)
    ok &= bool(cond)


def run(args):
    r = subprocess.run([make3do] + args, cwd=work, capture_output=True, text=True)
    return r.returncode, (r.stdout + r.stderr)


def read_vertex_lists(path):
    """Positions/normals of every PlainVertexListDescriptor in a .3do.

    The rts Archive writes a class name only the first time; later objects of
    the same class carry just its class id, so find those by id + header."""
    b = open(path, "rb").read()
    name = b"PlainVertexListDescriptor\0"
    first = b.find(name)
    if first < 0:
        return []
    class_id = struct.unpack_from("<i", b, first - 8)[0]
    header = b"\x01\0\0\0\0\0\0\0\x01\0\0\0\x01\0\0\0"  # base ver 1, null name, ver 1, 1
    starts = [first + len(name)]
    pat = struct.pack("<i", class_id) + header
    i = first + len(name)
    while True:
        i = b.find(pat, i)
        if i < 0:
            break
        starts.append(i + 4)
        i += len(pat)
    out = []
    for s in starts:
        o = s + 16
        n = struct.unpack_from("<i", b, o)[0]
        o += 4
        arrays = []
        for k in range(6):
            if k == 3:
                o += 4  # an int between positions and normals
            size = struct.unpack_from("<i", b, o)[0]
            arrays.append(struct.unpack_from(f"<{size // 8}d", b, o + 4))
            o += 4 + size
        out.append((n, arrays))
    return out


# 1. whole scene straight from the PAS
code, log = run(["-p", "scene.pas", "-o", "scene.3do"])
check(code == 0 and "ERROR" not in log, "make3do -p accepts the PAS")
if code != 0:
    print(log)
    sys.exit(1)

# 2. the PSG: one .3do per object, relative to its pivot
code, log = run(["-s", "scene.psg"])
check(code == 0 and "ERROR" not in log.upper().replace("ERRORS: 0", ""), "make3do -s accepts the generated PSG")
for n in ("Box", "Box_mirrored", "Box_moved"):
    check(os.path.isfile(os.path.join(work, f"{n}.3do")), f"{n}.3do written")
if code != 0:
    print(log)

# 3. hard edges survive: on flat cubes every output normal is axis-aligned (no averaging)
vl = read_vertex_lists(os.path.join(work, "scene.3do"))
normals = [tuple(a[k][v] for k in (3, 4, 5)) for n, a in vl for v in range(n)]
def is_axis(nv, eps=1e-6):
    s = sorted(abs(c) for c in nv)
    return s[0] < eps and s[1] < eps and abs(s[2] - 1) < eps


off = [nv for nv in normals if not is_axis(nv)]
check(vl and not off, f"flat shading kept: all {len(normals)} output normals are face normals" + (f" (e.g. {off[0]})" if off else ""))

# 4. winding: every face of the PAS faces the way its normals point, in world space
import re  # noqa: E402

text = open(pas, encoding="ascii").read()
bad = 0
for block in text.split("SHAPE {")[1:]:
    rows = [tuple(map(float, re.search(rf"XFORM_ROW{i} (\S+)\t(\S+)\t(\S+)", block).groups())) for i in range(3)]
    det = (rows[0][0] * (rows[1][1] * rows[2][2] - rows[1][2] * rows[2][1])
           - rows[0][1] * (rows[1][0] * rows[2][2] - rows[1][2] * rows[2][0])
           + rows[0][2] * (rows[1][0] * rows[2][1] - rows[1][1] * rows[2][0]))
    check(det > 0, "transform written right-handed (make3do rejects mirrored ones)")
    w = lambda p: tuple(p[0] * rows[0][k] + p[1] * rows[1][k] + p[2] * rows[2][k] for k in range(3))
    verts = [w(tuple(map(float, g))) for g in re.findall(r"VERTEX\s+\d+:\t(\S+)\t(\S+)\t(\S+)", block)]
    vn = {}
    for vi, x, y, z in re.findall(r"VERTEX_NORMAL (\d+)\t(\S+)\t(\S+)\t(\S+)", block):
        vn[int(vi)] = w((float(x), float(y), float(z)))
    for a, b_, c in re.findall(r"FACE \d+:\s+(\d+)\s+(\d+)\s+(\d+)", block):
        pa, pb, pc = verts[int(a)], verts[int(b_)], verts[int(c)]
        e1 = [pb[k] - pa[k] for k in range(3)]
        e2 = [pc[k] - pa[k] for k in range(3)]
        cr = (e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0])
        if sum(cr[k] * vn[int(a)][k] for k in range(3)) <= 0:
            bad += 1
check(bad == 0, f"every face is wound to face outward, mirrored object included ({bad} inverted)")

# 5. transforms in the combined scene (no pivots: world space)
allx = [x for _, a in vl for x in a[0]]
ally = [y for _, a in vl for y in a[1]]
allz = [z for _, a in vl for z in a[2]]
check(any(abs(z - 31) < 1e-6 for z in allz) and any(abs(y - 21) < 1e-6 for y in ally), "moved box lands at z 30..31, y 20..21")
check(any(abs(x - 9) < 1e-6 for x in allx), "rotated box spans x 9..10 (90 degrees about Z)")
check(any(abs(x - 4) < 1e-6 for x in allx), "mirrored box spans x 4..5")

# 6. per-object .3do with USEPIVOTS stays in local space
lv = read_vertex_lists(os.path.join(work, "Box_moved.3do"))
xs = [x for _, a in lv for x in a[0]]
zs = [z for _, a in lv for z in a[2]]
check(lv and max(zs) <= 1.000001 and min(zs) >= -1e-6, "USEPIVOTS: Box_moved.3do is in its own local space")

print("\nALL PASS" if ok else "\nSOME CHECKS FAILED")
sys.exit(0 if ok else 1)
