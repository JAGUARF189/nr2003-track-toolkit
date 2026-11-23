"""Blender add-on: export meshes as a Papyrus Ascii Scene (.pas) for NR2003's make3do.

File > Export > NR2003 Papyrus Scene (.pas)

Writes:
  <name>.pas            the scene, ready for make3do
  <name>.psg            optional make3do script: one .3do per object (trackside-object style)
  textures_src/*.png    optional copies of the images the materials use, named to match
                        the .mip names in the .pas (turn them into .mip with the Track Toolkit)
"""

bl_info = {
    "name": "NR2003 Papyrus Scene (.pas)",
    "author": "NR2003 Track Toolkit",
    "version": (0, 1, 0),
    "blender": (3, 6, 0),
    "location": "File > Export > NR2003 Papyrus Scene (.pas)",
    "description": "Export meshes for Papyrus make3do (NASCAR Racing 2003 track objects)",
    "category": "Import-Export",
}

import os
import shutil

if "bpy" in locals():
    import importlib

    importlib.reload(pas_writer)  # noqa: F821  (reload during development)
else:
    from . import pas_writer

import bpy
from bpy.props import BoolProperty, EnumProperty, StringProperty
from bpy_extras.io_utils import ExportHelper

CONVERTIBLE = {"MESH", "CURVE", "SURFACE", "FONT", "META"}


# ---------------------------------------------------------------- materials


def _upstream_image(node, seen=None):
    """First Image Texture node feeding `node` (searching back through mix/shader nodes)."""
    seen = seen or set()
    if node is None or node.name in seen:
        return None
    seen.add(node.name)
    if node.type == "TEX_IMAGE" and node.image is not None:
        return node.image
    for inp in node.inputs:
        for link in inp.links:
            img = _upstream_image(link.from_node, seen)
            if img is not None:
                return img
    return None


def _has_nodes(mat):
    # Material.use_nodes is deprecated in Blender 5.x and goes away in 6.0
    # (node materials become the only kind), so treat "missing" as True.
    return mat is not None and mat.node_tree is not None and getattr(mat, "use_nodes", True)


def _principled(mat):
    if not _has_nodes(mat):
        return None
    for n in mat.node_tree.nodes:
        if n.type == "BSDF_PRINCIPLED":
            return n
    return None


def base_image(mat):
    """The picture a material shows: Principled Base Color's image, else any image node."""
    if mat is None:
        return None
    p = _principled(mat)
    if p is not None:
        inp = p.inputs.get("Base Color")
        if inp is not None and inp.links:
            img = _upstream_image(inp.links[0].from_node)
            if img is not None:
                return img
    if _has_nodes(mat):
        for n in mat.node_tree.nodes:
            if n.type == "TEX_IMAGE" and n.image is not None:
                return n.image
    return None


def image_stem(img):
    path = img.filepath_raw or img.filepath or ""
    stem = os.path.splitext(os.path.basename(bpy.path.abspath(path)))[0] if path else ""
    return pas_writer.safe_name(stem or img.name)


def convert_material(mat):
    if mat is None:
        return pas_writer.DEFAULT_MATERIAL
    diffuse = tuple(mat.diffuse_color[:3])
    opacity = float(mat.diffuse_color[3])
    p = _principled(mat)
    if p is not None:
        bc = p.inputs.get("Base Color")
        if bc is not None and not bc.links:
            diffuse = tuple(bc.default_value[:3])
        al = p.inputs.get("Alpha")
        if al is not None and not al.links:
            opacity = float(al.default_value)
    img = base_image(mat)
    if img is not None:
        diffuse = (1.0, 1.0, 1.0)  # let the texture show its own colors
    return pas_writer.Material(
        name=mat.name,
        diffuse=diffuse,
        opacity=max(0.0, min(1.0, opacity)),
        base_texture=(image_stem(img) + ".mip") if img is not None else None,
    )


def copy_image(img, folder, report):
    """Copy (or unpack) a material's image as <stem>.<ext> into `folder`."""
    os.makedirs(folder, exist_ok=True)
    stem = image_stem(img)
    src = bpy.path.abspath(img.filepath_raw or img.filepath or "")
    if src and os.path.isfile(src):
        dst = os.path.join(folder, stem + os.path.splitext(src)[1].lower())
        if os.path.abspath(src) != os.path.abspath(dst):
            shutil.copyfile(src, dst)
        return dst
    if img.packed_file is not None:
        ext = {"PNG": ".png", "JPEG": ".jpg", "TARGA": ".tga", "BMP": ".bmp"}.get(img.file_format, ".png")
        dst = os.path.join(folder, stem + ext)
        with open(dst, "wb") as f:
            f.write(img.packed_file.data)
        return dst
    # Generated / painted-but-unsaved image: save a PNG copy.
    dst = os.path.join(folder, stem + ".png")
    try:
        img.save_render(dst)
        return dst
    except Exception as e:  # pragma: no cover - depends on Blender state
        report({"WARNING"}, f"Couldn't save image '{img.name}': {e}")
        return None


# ---------------------------------------------------------------- meshes


def convert_object(ob, depsgraph, apply_modifiers):
    ob_eval = ob.evaluated_get(depsgraph) if apply_modifiers else ob
    me = ob_eval.to_mesh()
    try:
        me.calc_loop_triangles()
        if hasattr(me, "calc_normals_split"):  # Blender < 4.1
            me.calc_normals_split()
        uv_layer = me.uv_layers.active
        positions = [tuple(v.co) for v in me.vertices]
        tris, normals, mats = [], [], []
        uvs = [] if uv_layer is not None else None
        for t in me.loop_triangles:
            tris.append(tuple(t.vertices))
            normals.append(tuple(tuple(n) for n in t.split_normals))
            mats.append(t.material_index)
            if uv_layer is not None:
                uvs.append(tuple(tuple(uv_layer.data[li].uv) for li in t.loops))
    finally:
        ob_eval.to_mesh_clear()

    m = ob.matrix_world
    return pas_writer.Mesh(
        name=ob.name,
        positions=positions,
        tris=tris,
        corner_normals=normals,
        corner_uvs=uvs,
        tri_material=mats,
        materials=[convert_material(s.material) for s in ob.material_slots],
        rows=tuple((m[0][i], m[1][i], m[2][i]) for i in range(3)),
        pos=tuple(m.translation),
    )


def unique_names(meshes):
    taken = set()
    for m in meshes:
        base = pas_writer.safe_name(m.name)
        name, n = base, 2
        while name.lower() in taken:
            name = f"{base}_{n}"
            n += 1
        taken.add(name.lower())
        m.name = name


# ---------------------------------------------------------------- operator


class EXPORT_OT_nr2003_pas(bpy.types.Operator, ExportHelper):
    """Export meshes for NR2003's make3do"""

    bl_idname = "export_scene.nr2003_pas"
    bl_label = "Export NR2003 PAS"
    bl_options = {"PRESET"}

    filename_ext = ".pas"
    filter_glob: StringProperty(default="*.pas", options={"HIDDEN"})

    use_selection: BoolProperty(name="Selected only", description="Export only selected objects", default=False)
    apply_modifiers: BoolProperty(name="Apply modifiers", default=True)
    write_psg: BoolProperty(
        name="Write .psg script",
        description="Also write a make3do script that makes one .3do per object, kept around its own origin "
        "(how trackside objects are placed in Sandbox)",
        default=True,
    )
    pivots: EnumProperty(
        name="Object positions",
        items=[
            ("PIVOTS", "Each around its own origin", "Trackside objects: Sandbox places them (USEPIVOTS)"),
            ("WORLD", "Where they are in the scene", "Keep scene positions in the .3do"),
        ],
        default="PIVOTS",
    )
    copy_images: BoolProperty(
        name="Copy pictures",
        description="Copy the images your materials use into a textures_src folder next to the .pas, "
        "named to match the .mip names (make them into .mip with the Track Toolkit)",
        default=True,
    )

    def execute(self, context):
        depsgraph = context.evaluated_depsgraph_get()
        objs = context.selected_objects if self.use_selection else context.view_layer.objects
        objs = [o for o in objs if o.type in CONVERTIBLE and o.visible_get()]
        if not objs:
            self.report({"ERROR"}, "Nothing to export: no visible mesh objects" + (" selected" if self.use_selection else ""))
            return {"CANCELLED"}

        meshes, skipped = [], []
        for ob in objs:
            try:
                m = convert_object(ob, depsgraph, self.apply_modifiers)
            except Exception as e:
                skipped.append(f"{ob.name} ({e})")
                continue
            if not m.tris:
                skipped.append(f"{ob.name} (no faces)")
                continue
            meshes.append(m)
        if not meshes:
            self.report({"ERROR"}, "Nothing to export: " + "; ".join(skipped))
            return {"CANCELLED"}
        unique_names(meshes)

        unit = context.scene.unit_settings.scale_length or 1.0
        try:
            text = pas_writer.build_pas(meshes, unit=unit, source=bpy.data.filepath or "unsaved Blender file")
        except pas_writer.ExportError as e:
            self.report({"ERROR"}, str(e))
            return {"CANCELLED"}

        path = self.filepath
        with open(path, "w", encoding="ascii", errors="replace", newline="\n") as f:
            f.write(text)
        folder = os.path.dirname(path)
        written = [os.path.basename(path)]

        if self.write_psg:
            psg = os.path.splitext(path)[0] + ".psg"
            with open(psg, "w", encoding="ascii", errors="replace", newline="\n") as f:
                f.write(pas_writer.build_psg(os.path.basename(path), meshes, use_pivots=self.pivots == "PIVOTS"))
            written.append(os.path.basename(psg))

        if self.copy_images:
            imgs = {}
            for ob in objs:
                for s in ob.material_slots:
                    img = base_image(s.material)
                    if img is not None:
                        imgs[img.name] = img
            for img in imgs.values():
                copy_image(img, os.path.join(folder, "textures_src"), self.report)
            if imgs:
                written.append(f"{len(imgs)} picture(s) in textures_src")

        tris = sum(len(m.tris) for m in meshes)
        msg = f"Exported {len(meshes)} object(s), {tris} triangles: " + ", ".join(written)
        if skipped:
            msg += f" — skipped: {'; '.join(skipped)}"
            self.report({"WARNING"}, msg)
        else:
            self.report({"INFO"}, msg)
        return {"FINISHED"}


def menu_func_export(self, context):
    self.layout.operator(EXPORT_OT_nr2003_pas.bl_idname, text="NR2003 Papyrus Scene (.pas)")


def register():
    bpy.utils.register_class(EXPORT_OT_nr2003_pas)
    bpy.types.TOPBAR_MT_file_export.append(menu_func_export)


def unregister():
    bpy.types.TOPBAR_MT_file_export.remove(menu_func_export)
    bpy.utils.unregister_class(EXPORT_OT_nr2003_pas)
