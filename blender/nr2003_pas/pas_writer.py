"""Papyrus Ascii Scene (.pas) writer for NR2003's make3do. Pure Python, no Blender.

The grammar was recovered from Papyrus's 3ds Max exporter (maxascii.dle) and
checked against make3do.exe, which is the only consumer that matters:

    VERSION 200                         make3do rejects anything else
    UNIT_CONVERSION <metres per unit>   multiplies every coordinate
    SHAPE {
        NAME "<name>"
        TRANSFORM {
            XFORM_ROW0..2 <x y z>       image of the X/Y/Z axis (3ds Max row convention)
            XFORM_ROW3 <x y z>          written for completeness; make3do ignores it
            XFORM_POS <x y z>           translation make3do actually applies
        }
        MESH {
            NUM_VERTICES n   VERTEX_LIST { VERTEX i: x y z ... }        object-local coords
            NUM_FACES m      FACE_LIST { FACE i: a b c ... }
            NORMAL_LIST { FACE_NORMAL i: 0 nx ny nz
                              VERTEX_NORMAL <vertex index> nx ny nz  (x3) ... }
            NUM_MATERIAL_REFS k
            MATERIAL_REF {
                MATERIAL { NAME, MATERIAL_AMBIENT/DIFFUSE/SPECULAR r g b,
                           MATERIAL_SHININESS/REFLECTIVITY/OPACITY v,
                           [BASE_TEXTURE "x.mip"] [ENVIRONMENT_MAP_TEXTURE "y.mip"] }
                NUM_FACES j
                per face: TEXTURE_FACE <face> BASE_TEXTURE (u, v) (u, v) (u, v)   if textured
                       or MAT_FACE <face>                                         if not
            }
        }
    }

make3do keeps exactly one normal and UV per PAS vertex (it never splits), so
this writer splits vertices itself wherever a corner's normal or UV differs.
Coordinates, winding (counter-clockwise = front) and UVs (V up) follow the
3ds Max conventions, which are also Blender's.
"""

from __future__ import annotations

import math
import re
from dataclasses import dataclass, field
from typing import List, Optional, Sequence, Tuple

Vec3 = Tuple[float, float, float]
Vec2 = Tuple[float, float]


@dataclass
class Material:
    name: str
    diffuse: Vec3 = (0.8, 0.8, 0.8)
    ambient: Optional[Vec3] = None  # defaults to diffuse
    specular: Vec3 = (0.0, 0.0, 0.0)
    shininess: float = 0.0
    reflectivity: float = 0.0
    opacity: float = 1.0
    base_texture: Optional[str] = None  # e.g. "sign.mip"
    environment_texture: Optional[str] = None


@dataclass
class Mesh:
    name: str
    positions: List[Vec3]
    tris: List[Tuple[int, int, int]]
    corner_normals: List[Tuple[Vec3, Vec3, Vec3]]
    tri_material: List[int]
    materials: List[Material]
    corner_uvs: Optional[List[Tuple[Vec2, Vec2, Vec2]]] = None
    # Object transform: rows[i] = where local axis i ends up; pos = translation.
    rows: Tuple[Vec3, Vec3, Vec3] = ((1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0))
    pos: Vec3 = (0.0, 0.0, 0.0)


DEFAULT_MATERIAL = Material(name="default_gray", diffuse=(0.6, 0.6, 0.6))


def safe_name(name: str) -> str:
    """Names make3do/PSG scripts can use unquoted: letters, digits, underscore."""
    s = re.sub(r"[^A-Za-z0-9_]", "_", name.strip())
    if not s:
        s = "object"
    if s[0].isdigit():
        s = "_" + s
    return s


def _f(v: float) -> str:
    if not math.isfinite(v):
        raise ValueError(f"non-finite number {v}")
    s = f"{v:.6f}"
    return "0.000000" if s == "-0.000000" else s


def _v3(v: Sequence[float]) -> str:
    return "\t".join(_f(x) for x in v)


def _normalize(v: Sequence[float]) -> Vec3:
    l = math.sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
    if l < 1e-12:
        return (0.0, 0.0, 1.0)
    return (v[0] / l, v[1] / l, v[2] / l)


def _det3(r: Tuple[Vec3, Vec3, Vec3]) -> float:
    (a, b, c), (d, e, f), (g, h, i) = r
    return a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g)


def _key(p_index: int, n: Vec3, uv: Optional[Vec2]) -> tuple:
    q = lambda x: round(x, 5)
    return (p_index, q(n[0]), q(n[1]), q(n[2])) + ((q(uv[0]), q(uv[1])) if uv else ())


class ExportError(ValueError):
    pass


def _check(m: Mesh) -> None:
    n = len(m.tris)
    if not m.positions:
        raise ExportError(f"'{m.name}' has no vertices")
    if n == 0:
        raise ExportError(f"'{m.name}' has no faces")
    if len(m.corner_normals) != n or len(m.tri_material) != n:
        raise ExportError(f"'{m.name}': per-face data doesn't match the face count")
    if m.corner_uvs is not None and len(m.corner_uvs) != n:
        raise ExportError(f"'{m.name}': UV data doesn't match the face count")
    for t in m.tris:
        if any(i < 0 or i >= len(m.positions) for i in t):
            raise ExportError(f"'{m.name}': a face uses a vertex that doesn't exist")


def build_mesh_block(m: Mesh, indent: str = "\t") -> List[str]:
    """Lines for one SHAPE { ... } block."""
    _check(m)
    mats = list(m.materials) or [DEFAULT_MATERIAL]
    # make3do rejects left-handed (mirrored) transforms. Bake the mirror into the
    # mesh instead: negate local X of positions and normals and of the X axis row
    # (so world positions are unchanged), and reverse winding to keep faces front-facing.
    mirrored = _det3(m.rows) < 0
    rows = m.rows
    positions = m.positions
    if mirrored:
        rows = (tuple(-x for x in m.rows[0]), m.rows[1], m.rows[2])
        positions = [(-p[0], p[1], p[2]) for p in m.positions]

    # UVs only matter (and only cause splits) on faces whose material has a texture.
    def _mat(mi):
        return mats[mi] if 0 <= mi < len(mats) else mats[0]

    textured = [bool(_mat(mi).base_texture) for mi in m.tri_material]

    # Split vertices per unique (position, normal, uv) corner.
    verts: List[Vec3] = []
    vnorm: List[Vec3] = []
    lookup = {}
    faces: List[Tuple[int, int, int]] = []
    face_uvs: List[Optional[Tuple[Vec2, Vec2, Vec2]]] = []
    for ti, tri in enumerate(m.tris):
        order = (0, 2, 1) if mirrored else (0, 1, 2)
        idx = []
        uvs = []
        for c in order:
            p = tri[c]
            n = _normalize(m.corner_normals[ti][c])
            if mirrored:
                n = (-n[0], n[1], n[2])
            uv = m.corner_uvs[ti][c] if m.corner_uvs is not None and textured[ti] else None
            k = _key(p, n, uv)
            vi = lookup.get(k)
            if vi is None:
                vi = len(verts)
                lookup[k] = vi
                verts.append(tuple(positions[p]))
                vnorm.append(n)
            idx.append(vi)
            uvs.append(uv)
        faces.append(tuple(idx))
        face_uvs.append(tuple(uvs) if m.corner_uvs is not None and textured[ti] else None)

    i1, i2, i3, i4 = indent, indent * 2, indent * 3, indent * 4
    L = [
        "SHAPE {",
        f'{i1}NAME "{safe_name(m.name)}"',
        f"{i1}TRANSFORM {{",
        f"{i2}XFORM_ROW0 {_v3(rows[0])}",
        f"{i2}XFORM_ROW1 {_v3(rows[1])}",
        f"{i2}XFORM_ROW2 {_v3(rows[2])}",
        f"{i2}XFORM_ROW3 {_v3(m.pos)}",
        f"{i2}XFORM_POS {_v3(m.pos)}",
        f"{i1}}}",
        f"{i1}MESH {{",
        f"{i2}NUM_VERTICES {len(verts)}",
        f"{i2}VERTEX_LIST {{",
    ]
    L += [f"{i3}VERTEX {i:4d}:\t{_v3(v)}" for i, v in enumerate(verts)]
    L += [f"{i2}}}", f"{i2}NUM_FACES {len(faces)}", f"{i2}FACE_LIST {{"]
    L += [f"{i3}FACE {i}: {a:4d} {b:4d} {c:4d}" for i, (a, b, c) in enumerate(faces)]
    L += [f"{i2}}}", f"{i2}NORMAL_LIST {{"]
    for i, (a, b, c) in enumerate(faces):
        pa, pb, pc = verts[a], verts[b], verts[c]
        e1 = (pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2])
        e2 = (pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2])
        fn = _normalize((e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]))
        L.append(f"{i3}FACE_NORMAL {i}: 0 {_v3(fn)}")
        for v in (a, b, c):
            L.append(f"{i4}VERTEX_NORMAL {v}\t{_v3(vnorm[v])}")
    L.append(f"{i2}}}")

    # Faces grouped by material, in material order; unused materials are skipped.
    groups = {}
    for fi, mi in enumerate(m.tri_material):
        mi = mi if 0 <= mi < len(mats) else 0
        groups.setdefault(mi, []).append(fi)
    used = sorted(groups)
    L.append(f"{i2}NUM_MATERIAL_REFS {len(used)}")
    for mi in used:
        mat = mats[mi]
        amb = mat.ambient if mat.ambient is not None else mat.diffuse
        L += [
            f"{i2}MATERIAL_REF {{",
            f"{i3}MATERIAL {{",
            f'{i4}NAME "{safe_name(mat.name)}"',
            f"{i4}MATERIAL_AMBIENT {_v3(amb)}",
            f"{i4}MATERIAL_DIFFUSE {_v3(mat.diffuse)}",
            f"{i4}MATERIAL_SPECULAR {_v3(mat.specular)}",
            f"{i4}MATERIAL_SHININESS {_f(mat.shininess)}",
            f"{i4}MATERIAL_REFLECTIVITY {_f(mat.reflectivity)}",
            f"{i4}MATERIAL_OPACITY {_f(mat.opacity)}",
        ]
        if mat.base_texture:
            L.append(f'{i4}BASE_TEXTURE "{mat.base_texture}"')
        if mat.environment_texture:
            L.append(f'{i4}ENVIRONMENT_MAP_TEXTURE "{mat.environment_texture}"')
        L.append(f"{i3}}}")
        L.append(f"{i3}NUM_FACES {len(groups[mi])}")
        for fi in groups[mi]:
            uv = face_uvs[fi]
            if mat.base_texture:
                uv = uv or ((0.0, 0.0), (0.0, 0.0), (0.0, 0.0))
                coords = " ".join(f"({_f(u)}, {_f(v)})" for u, v in uv)
                L.append(f"{i3}TEXTURE_FACE  {fi} BASE_TEXTURE {coords}")
            else:
                L.append(f"{i3}MAT_FACE  {fi}")
        L.append(f"{i2}}}")
    L += [f"{i1}}}", "}"]
    return L


def build_pas(meshes: Sequence[Mesh], unit: float = 1.0, source: str = "") -> str:
    names = [safe_name(m.name) for m in meshes]
    dupes = {n for n in names if names.count(n) > 1}
    if dupes:
        raise ExportError("two objects end up with the same name: " + ", ".join(sorted(dupes)))
    out = [
        "#############################################",
        "#",
        "# Papyrus Ascii Scene written by the NR2003 Track Toolkit Blender exporter.",
        f"# Source: {source}" if source else "#",
        "#",
        "#############################################",
        "VERSION\t200",
        f"UNIT_CONVERSION\t{_f(unit)}",
    ]
    for m in meshes:
        out += build_mesh_block(m)
    return "\n".join(out) + "\n"


def build_psg(pas_file: str, meshes: Sequence[Mesh], use_pivots: bool = True) -> str:
    """A make3do script that turns every object into its own .3do (TSO style)."""
    lines = [
        "# make3do script written by the NR2003 Track Toolkit Blender exporter.",
        f'pasgroup: MESH_GROUP "{pas_file}"' + (" USEPIVOTS" if use_pivots else ""),
    ]
    for m in meshes:
        n = safe_name(m.name)
        lines.append(f"m_{n}: MESH pasgroup {n}")
        lines.append(f'OUTPUT m_{n} "{n}.3do"')
    return "\n".join(lines) + "\n"
