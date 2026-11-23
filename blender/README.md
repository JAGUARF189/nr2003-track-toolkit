# NR2003 Papyrus Scene (.pas) exporter for Blender

This exporter replaces 3ds Max 2.5 and `maxascii.dle`. It writes `.pas` files that Papyrus `make3do` turns into `.3do` track objects.

## Install

1. Zip the `nr2003_pas` folder, so you have `nr2003_pas.zip` with the folder inside it.
2. In Blender, go to **Edit → Preferences → Add-ons**, choose **Install from Disk…** (or **Install…** in older versions), pick the zip, and tick **NR2003 Papyrus Scene (.pas)**.
3. Export from **File → Export → NR2003 Papyrus Scene (.pas)**.

## What it writes

| File | What it is |
|---|---|
| `name.pas` | The scene for make3do. |
| `name.psg` | A make3do script that makes one `.3do` per object, each kept around its own origin. This is how trackside objects are placed in Sandbox. The Track Toolkit's **PSG → 3DO** step runs it. |
| `textures_src/` | Copies of the pictures your materials use, named to match the `.mip` names in the `.pas`. Turn them into `.mip` with the Track Toolkit (Textures tab → Add pictures, or the Build track Textures step). |

## How Blender maps to NR2003

- **Units:** 1 Blender unit is 1 metre. The scene's Unit Scale goes into `UNIT_CONVERSION`.
- **Axes:** Z is up. Blender and 3ds Max use the same axes, so nothing is converted.
- **Faces:** n-gons and quads are triangulated. Modifiers are applied (this can be turned off).
- **Shading:** Smooth/Flat shading, sharp edges and custom normals carry over. Vertices are split wherever normals or UVs differ, because make3do keeps only one normal and UV per vertex.
- **Mirrored objects** (negative scale) are fixed automatically. make3do rejects mirrored transforms, so the mirror is built into the mesh instead.
- **Materials:** each material slot becomes a Papyrus material. The picture comes from the Principled BSDF **Base Color** image, or the first Image Texture node if there isn't one. The texture name is `<image file name>.mip`. Base color and Alpha become diffuse color and opacity.
- **UVs:** the active UV map is used.

## The PAS format (as checked against make3do)

The grammar and rules are in the comment at the top of `nr2003_pas/pas_writer.py`. The main points:

- make3do only accepts `VERSION 200`.
- Objects are `SHAPE { NAME, TRANSFORM, MESH }`.
- make3do applies `XFORM_POS` and ignores `XFORM_ROW3`.
- A `VERTEX_NORMAL n` line takes a vertex index.
- Inside a `MATERIAL_REF`, each face is written as either `TEXTURE_FACE <face> BASE_TEXTURE (u, v)×3` (textured) or `MAT_FACE <face>` (not textured).

## Tests

- `tests/test_pas_writer.py` needs no Blender. It builds scenes, compiles them with `make3do -p` and `-s`, and checks the `.3do` output: transforms, flat shading, winding and USEPIVOTS.

  ```
  python tests/test_pas_writer.py <work dir> "C:\Program Files (x86)\sandbox\make3do.exe" <any .mip>
  ```
- `tests/blender_e2e.py` runs inside Blender with no window (`blender -b --python …`). It goes through the real export operator.
