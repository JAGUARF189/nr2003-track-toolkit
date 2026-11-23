"""End-to-end test inside Blender (headless):

  blender -b --factory-startup --python blender_e2e.py -- <addon parent dir> <out dir>

Builds a scene (smooth + flat shading, modifiers, UV-mapped image texture,
a mirrored object, an n-gon), exports it with the add-on's operator, and
leaves scene.pas / scene.psg / textures_src for make3do to check.
"""
import os
import sys

import bpy

argv = sys.argv[sys.argv.index("--") + 1:]
addon_parent, out = argv[0], argv[1]
os.makedirs(out, exist_ok=True)
sys.path.insert(0, addon_parent)
import nr2003_pas  # noqa: E402

nr2003_pas.register()

bpy.ops.wm.read_factory_settings(use_empty=True)

# A picture for the textured material.
img = bpy.data.images.new("SignArt", 64, 32, alpha=True)
img.pixels = [c for y in range(32) for x in range(64) for c in ((x / 63), (y / 31), 0.2, 1.0)]
img.filepath_raw = os.path.join(out, "SignArt.png")
img.file_format = "PNG"
img.save()

mat = bpy.data.materials.new("Sign Material")
mat.use_nodes = True
bsdf = mat.node_tree.nodes["Principled BSDF"]
tex = mat.node_tree.nodes.new("ShaderNodeTexImage")
tex.image = img
mat.node_tree.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])

grey = bpy.data.materials.new("Concrete")
grey.diffuse_color = (0.5, 0.5, 0.5, 1.0)

# 1. Textured, flat-shaded cube with a bevel modifier, two materials.
bpy.ops.mesh.primitive_cube_add(size=2, location=(0, 0, 1))
cube = bpy.context.active_object
cube.name = "Sign Board"
cube.data.materials.append(mat)
cube.data.materials.append(grey)
for i, p in enumerate(cube.data.polygons):
    p.material_index = 0 if i < 2 else 1
cube.modifiers.new("bevel", "BEVEL").width = 0.1

# 2. Smooth sphere, mirrored with negative scale and rotated.
bpy.ops.mesh.primitive_uv_sphere_add(segments=16, ring_count=8, location=(6, 0, 1))
sph = bpy.context.active_object
sph.name = "Ball (mirrored)"
bpy.ops.object.shade_smooth()
sph.scale = (-1.0, 1.0, 1.0)
sph.rotation_euler = (0, 0, 0.7)
sph.data.materials.append(grey)

# 3. A single n-gon (pentagon) — must be triangulated.
bpy.ops.mesh.primitive_circle_add(vertices=5, fill_type="NGON", location=(0, 6, 0))
bpy.context.active_object.name = "Pentagon"

bpy.context.view_layer.update()
res = bpy.ops.export_scene.nr2003_pas(filepath=os.path.join(out, "scene.pas"), write_psg=True, copy_images=True)
print("EXPORT RESULT", res)
