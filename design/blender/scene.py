"""RigSpark mascot ("Sparky", placeholder name) — procedural 3D scenes.

Usage:  python3 scene.py <shot> <out.png> [res] [samples] [save.blend]
Shots:  hero | wave | chip | peek | verdicts
Renders with Cycles CPU, transparent film + shadow catcher -> web-ready PNG.
"""
import sys, math
import bpy, bmesh
from mathutils import Vector

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else sys.argv[1:]
SHOT = argv[0] if argv else "hero"
OUT = __import__("os").path.abspath(argv[1]) if len(argv) > 1 else f"/tmp/{SHOT}.png"
RES = int(argv[2]) if len(argv) > 2 else 600
SAMPLES = int(argv[3]) if len(argv) > 3 else 16
BLEND = argv[4] if len(argv) > 4 else None

def hex2lin(h):
    h = h.lstrip("#"); c = [int(h[i:i+2], 16) / 255 for i in (0, 2, 4)]
    return tuple(((x + 0.055) / 1.055) ** 2.4 if x > 0.04045 else x / 12.92 for x in c) + (1.0,)

GRAPHITE = "#3e3f45"; GRAPHITE_D = "#232428"; AMBER = "#ffad00"; PEACH = "#f9b08b"
CREAM = "#fff3e8"; YES = "#6cc24a"; SLOW = "#ffb21a"; NO = "#ff6b55"; TEAL = "#1a959b"

# ---------------------------------------------------------------- reset
bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
col = scene.collection

def link(obj):
    col.objects.link(obj); return obj

def mesh_obj(name, bm):
    me = bpy.data.meshes.new(name); bm.to_mesh(me); bm.free()
    return link(bpy.data.objects.new(name, me))

def smooth(obj, sub=2, bevel=None, segs=6):
    if bevel:
        m = obj.modifiers.new("bevel", "BEVEL"); m.width = bevel; m.segments = segs; m.limit_method = "NONE"
    if sub:
        m = obj.modifiers.new("sub", "SUBSURF"); m.levels = sub; m.render_levels = sub
    for p in obj.data.polygons: p.use_smooth = True
    return obj

def box(name, size, loc=(0, 0, 0), bevel=0.1, sub=2):
    bm = bmesh.new(); bmesh.ops.create_cube(bm, size=1.0)
    bmesh.ops.scale(bm, vec=Vector(size), verts=bm.verts)
    o = mesh_obj(name, bm); o.location = loc
    return smooth(o, sub, bevel)

def sphere(name, r, loc, scale=(1, 1, 1)):
    bm = bmesh.new(); bmesh.ops.create_uvsphere(bm, u_segments=32, v_segments=16, radius=r)
    o = mesh_obj(name, bm); o.location = loc; o.scale = scale
    for p in o.data.polygons: p.use_smooth = True
    return o

def capsule(name, r, length, loc, rot=(0, 0, 0)):
    o = sphere(name, r, loc, (1.0, 0.9, length / (2 * r))); o.rotation_euler = rot
    return o

def bolt(name, scale, loc, rot=(math.radians(90), 0, 0), depth=0.04):
    pts = [(0.15, 1.0), (-0.45, -0.05), (-0.02, -0.05), (-0.2, -1.0), (0.45, 0.12), (0.03, 0.12), (0.32, 1.0)]
    bm = bmesh.new()
    vs = [bm.verts.new((x, y, 0)) for x, y in pts]
    f = bm.faces.new(vs)
    ext = bmesh.ops.extrude_face_region(bm, geom=[f])
    bmesh.ops.translate(bm, vec=(0, 0, depth / scale), verts=[v for v in ext["geom"] if isinstance(v, bmesh.types.BMVert)])
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
    o = mesh_obj(name, bm); o.scale = (scale, scale, scale); o.location = loc; o.rotation_euler = rot
    m = o.modifiers.new("bevel", "BEVEL"); m.width = 0.06; m.segments = 4; m.limit_method = "NONE"
    for p in o.data.polygons: p.use_smooth = True
    return o

def arc(name, r, thick, loc, start=200, end=340):
    bm = bmesh.new(); segs = 20; ring = 10; verts = []
    for i in range(segs + 1):
        a = math.radians(start + (end - start) * i / segs)
        c = Vector((math.cos(a) * r, 0, math.sin(a) * r)); t = Vector((math.cos(a), 0, math.sin(a)))
        row = []
        for j in range(ring):
            b = 2 * math.pi * j / ring
            row.append(bm.verts.new(c + t * math.cos(b) * thick + Vector((0, 1, 0)) * math.sin(b) * thick))
        verts.append(row)
    for i in range(segs):
        for j in range(ring):
            bm.faces.new([verts[i][j], verts[i][(j + 1) % ring], verts[i + 1][(j + 1) % ring], verts[i + 1][j]])
    o = mesh_obj(name, bm); o.location = loc
    for p in o.data.polygons: p.use_smooth = True
    return o

# ---------------------------------------------------------------- materials
def mat(name, color, rough=0.5, emit=0.0, sheen=0.0, coat=0.0):
    m = bpy.data.materials.new(name); m.use_nodes = True
    b = m.node_tree.nodes["Principled BSDF"]
    b.inputs["Base Color"].default_value = hex2lin(color)
    b.inputs["Roughness"].default_value = rough
    b.inputs["Coat Weight"].default_value = coat
    b.inputs["Sheen Weight"].default_value = sheen
    if emit:
        b.inputs["Emission Color"].default_value = hex2lin(color)
        b.inputs["Emission Strength"].default_value = emit
    return m

def soft_plastic(name, color, rough=0.55):
    """Matte soft-touch plastic with a faint noise bump (keeps it tactile, not CG-perfect)."""
    m = mat(name, color, rough, sheen=0.25, coat=0.12)
    nt = m.node_tree; b = nt.nodes["Principled BSDF"]
    n = nt.nodes.new("ShaderNodeTexNoise"); n.inputs["Scale"].default_value = 180; n.inputs["Detail"].default_value = 2
    bp = nt.nodes.new("ShaderNodeBump"); bp.inputs["Strength"].default_value = 0.03
    nt.links.new(n.outputs["Fac"], bp.inputs["Height"]); nt.links.new(bp.outputs["Normal"], b.inputs["Normal"])
    return m

M_BODY = soft_plastic("body", GRAPHITE, 0.5)
M_DARK = mat("dark", GRAPHITE_D, 0.35)
M_VISOR = mat("visor", AMBER, 0.38, coat=0.25)
M_SPARK = mat("spark", "#ff9a00", 0.35, emit=0.6)
M_YES = soft_plastic("yes", YES, 0.4); M_SLOW = soft_plastic("slow", SLOW, 0.4); M_NO = soft_plastic("no", NO, 0.4)
M_VISOR_SLOW = mat("visorslow", "#FFC93C", 0.38, coat=0.25); M_VISOR_NO = mat("visorno", "#FF5A45", 0.38, coat=0.25)
M_CREAM = soft_plastic("cream", CREAM, 0.45); M_TEAL = soft_plastic("teal", TEAL, 0.45)
M_GOLD = mat("gold", "#e7b75a", 0.25); M_GOLD.node_tree.nodes["Principled BSDF"].inputs["Metallic"].default_value = 1.0

def setmat(o, m):
    o.data.materials.clear(); o.data.materials.append(m); return o

# ---------------------------------------------------------------- mascot
def mascot(root_loc=(0, 0, 0), pose="wave", tilt=0.0, look=0.0, expr="yes"):
    """Builds Sparky; returns an empty that parents every part."""
    root = link(bpy.data.objects.new("Sparky", None)); root.location = root_loc
    parts = []
    W, D, H = 1.0, 0.78, 1.32
    body = setmat(box("body", (W, D, H), (0, 0, H / 2 + 0.06), bevel=0.42, sub=3), M_BODY); parts.append(body)
    # visor: a soft pill that wraps the front
    vz = 0.98
    visor = setmat(box("visor", (0.84, 0.22, 0.4), (0, -D / 2 + 0.06, vz), bevel=0.17, sub=3), {"yes": M_VISOR, "slow": M_VISOR_SLOW, "no": M_VISOR_NO}[expr])
    bend = visor.modifiers.new("bend", "SIMPLE_DEFORM"); bend.deform_method = "BEND"; bend.deform_axis = "Z"
    bend.angle = math.radians(-38)
    visor.modifiers.move(len(visor.modifiers) - 1, 0)
    parts.append(visor)
    fy = -D / 2 - 0.165
    for x, sy in ((-0.2, 1.0), (0.13, 0.92)):
        ey = fy - 0.004 + abs(x) * 0.12
        if expr == "yes":
            parts.append(setmat(sphere("eye", 0.055, (x, ey, vz - 0.02), (1, 0.6, 1.35 * sy)), M_DARK))
        elif expr == "slow":   # sleepy: flat lids
            parts.append(setmat(sphere("eye", 0.055, (x, ey, vz - 0.03), (1.45, 0.6, 0.32)), M_DARK))
        else:                  # no: X eyes
            for a in (45, -45):
                e = setmat(sphere("eye", 0.05, (x, ey, vz - 0.02), (0.28, 0.6, 1.45)), M_DARK); e.rotation_euler = (0, math.radians(a), 0); parts.append(e)
    if expr == "yes":
        parts.append(setmat(arc("smile", 0.05, 0.013, (-0.035, fy - 0.006, vz - 0.03)), M_DARK))
    elif expr == "slow":
        parts.append(setmat(sphere("mouth", 0.05, (-0.035, fy - 0.004, vz - 0.085), (0.55, 0.5, 0.2)), M_DARK))
    else:
        parts.append(setmat(arc("frown", 0.045, 0.013, (-0.035, fy - 0.006, vz - 0.12), 25, 155), M_DARK))
    b = setmat(bolt("visorbolt", 0.075, (0.31, fy + 0.012, vz + 0.06), depth=0.02), M_DARK); parts.append(b)
    # stubby feet
    for x in (-0.24, 0.24):
        f = setmat(box("foot", (0.26, 0.34, 0.14), (x, -0.04, 0.07), bevel=0.065, sub=2), M_DARK); parts.append(f)
    # arms
    if pose == "wave":
        la = setmat(capsule("armL", 0.08, 0.36, (-0.52, -0.02, 0.6), (0, math.radians(14), 0)), M_BODY)
        ra = setmat(capsule("armR", 0.08, 0.40, (0.6, -0.04, 1.08), (math.radians(-8), math.radians(36), 0)), M_BODY)
    elif pose == "hold":
        la = setmat(capsule("armL", 0.08, 0.36, (-0.4, -0.32, 0.52), (math.radians(70), math.radians(35), 0)), M_BODY)
        ra = setmat(capsule("armR", 0.08, 0.36, (0.4, -0.32, 0.52), (math.radians(70), math.radians(-35), 0)), M_BODY)
    elif pose == "rest":
        la = setmat(capsule("armL", 0.08, 0.36, (-0.53, -0.02, 0.58), (0, math.radians(12), 0)), M_BODY)
        ra = setmat(capsule("armR", 0.08, 0.36, (0.53, -0.02, 0.58), (0, math.radians(-12), 0)), M_BODY)
    else:  # cheer: both up
        la = setmat(capsule("armL", 0.085, 0.42, (-0.66, -0.06, 0.92), (0, math.radians(-58), 0)), M_BODY)
        ra = setmat(capsule("armR", 0.085, 0.42, (0.66, -0.06, 0.92), (0, math.radians(58), 0)), M_BODY)
    parts += [la, ra]
    for p in parts:
        p.parent = root
    root.rotation_euler = (0, math.radians(tilt), math.radians(look))
    return root

def spark(loc, s=0.12, rz=0.0, ry=15):
    o = setmat(bolt("spark", s, loc, (math.radians(90), math.radians(ry), math.radians(rz)), depth=0.05), M_SPARK)
    o.visible_shadow = False
    return o

def pill(loc, m, size=(0.5, 0.12, 0.2), rot=(0, 0, 0)):
    o = setmat(box("pill", size, loc, bevel=min(size) * 0.48, sub=2), m); o.rotation_euler = rot; return o

def chip(loc, rot=(0, 0, 0), s=1.0):
    root = link(bpy.data.objects.new("chip", None)); root.location = loc; root.rotation_euler = rot; root.scale = (s, s, s)
    base = setmat(box("chipbase", (0.62, 0.62, 0.08), (0, 0, 0), bevel=0.04), M_DARK); base.parent = root
    die = setmat(box("die", (0.3, 0.3, 0.05), (0, 0, 0.055), bevel=0.02), M_VISOR); die.parent = root
    for i in range(5):
        t = -0.2 + i * 0.1
        for (x, y, sx, sy) in ((t, 0.36, 0.035, 0.12), (t, -0.36, 0.035, 0.12), (0.36, t, 0.12, 0.035), (-0.36, t, 0.12, 0.035)):
            p = setmat(box("pin", (sx, sy, 0.025), (x, y, 0), bevel=0.01, sub=1), M_GOLD); p.parent = root
    return root

# ---------------------------------------------------------------- shots
cam_data = bpy.data.cameras.new("cam"); cam = link(bpy.data.objects.new("cam", cam_data)); scene.camera = cam

def aim(loc, target, lens=50):
    cam.location = loc; cam_data.lens = lens
    d = Vector(target) - Vector(loc); cam.rotation_euler = d.to_track_quat("-Z", "Y").to_euler()

aspect = (1, 1)
if SHOT == "hero":
    mascot((0, 0, 0), "wave", look=12)
    spark((-0.95, -0.3, 1.55), 0.13, 10, 20); spark((1.15, -0.1, 1.75), 0.1, -15, -10); spark((-0.8, -0.5, 0.45), 0.08, 30, 25)
    pill((1.05, -0.45, 0.42), M_YES, (0.42, 0.13, 0.17), (0, math.radians(-10), math.radians(-14)))
    pill((-1.08, -0.2, 1.0), M_SLOW, (0.36, 0.12, 0.15), (0, math.radians(12), math.radians(18)))
    chip((1.0, 0.3, 1.25), (math.radians(60), math.radians(-20), math.radians(25)), 0.55)
    aim((0.5, -4.6, 1.45), (0.05, 0, 0.9), 50); aspect = (5, 4)
elif SHOT == "wave":
    mascot((0, 0, 0), "wave", look=-8)
    spark((0.95, -0.2, 1.65), 0.1, -10, -10)
    aim((0.3, -4.6, 1.3), (0.05, 0, 0.8), 50)
elif SHOT == "cheer":
    mascot((0, 0, 0), "cheer", look=0)
    spark((-0.95, -0.2, 1.55), 0.11, 10, 20); spark((0.95, -0.2, 1.6), 0.11, -10, -15)
    aim((0.0, -4.8, 1.3), (0, 0, 0.85), 50)
elif SHOT == "chip":
    mascot((0, 0, 0), "hold", look=-6)
    chip((0, -0.6, 0.5), (math.radians(72), 0, math.radians(-4)), 0.55)
    spark((0.75, -0.6, 1.45), 0.09, -10, -10)
    aim((0.35, -4.6, 1.35), (0, 0, 0.8), 50)
elif SHOT == "peek":
    mascot((0, 0, -0.62), "cheer", look=0)
    aim((0.0, -4.2, 0.85), (0, 0, 0.6), 55); aspect = (16, 9)
elif SHOT in ("studio", "turn"):
    import os
    ang = float(os.environ.get("ANGLE", "-18")) if SHOT == "turn" else -18
    mascot((0, 0, 0), "rest", look=ang, expr=os.environ.get("EXPR", "yes"))
    aim((0.0, -4.7, 1.0), (0, 0, 0.74), 70)
elif SHOT == "lineup":
    for x, e, lk in ((-1.3, "yes", 14), (0, "slow", 0), (1.3, "no", -14)):
        mascot((x, 0, 0), "rest", look=lk, expr=e)
    aim((0.0, -8.6, 1.2), (0, 0, 0.72), 60); aspect = (2, 1)
elif SHOT == "verdicts":
    pill((-0.62, 0, 0.12), M_YES, (0.5, 0.24, 0.24)); pill((0, 0, 0.12), M_SLOW, (0.5, 0.24, 0.24)); pill((0.62, 0, 0.12), M_NO, (0.5, 0.24, 0.24))
    aim((0.0, -3.0, 1.1), (0, 0, 0.12), 60); aspect = (2, 1)

# ground: shadow catcher
bm = bmesh.new(); bmesh.ops.create_grid(bm, x_segments=1, y_segments=1, size=12)
ground = mesh_obj("ground", bm); ground.is_shadow_catcher = True
if SHOT == "peek": ground.hide_render = True

# ---------------------------------------------------------------- lights & world
def area(name, loc, power, size, color=(1, 1, 1), target=(0, 0, 0.8)):
    l = bpy.data.lights.new(name, "AREA"); l.energy = power; l.size = size; l.color = color
    o = link(bpy.data.objects.new(name, l)); o.location = loc
    o.rotation_euler = (Vector(target) - Vector(loc)).to_track_quat("-Z", "Y").to_euler(); return o

STUDIO = SHOT in ("studio", "turn", "lineup")
if STUDIO:   # neutral product-photography rig: big soft top/front key, two rims for edge definition
    area("key", (-1.2, -3.6, 4.6), 520, 5.0, (1.0, 0.98, 0.96))
    area("fill", (3.8, -2.8, 1.4), 110, 4.0, (1.0, 1.0, 1.0))
    area("rimL", (-2.8, 2.6, 2.4), 360, 1.6, (1.0, 0.95, 0.9))
    area("rimR", (2.8, 2.6, 2.4), 360, 1.6, (1.0, 0.95, 0.9))
else:
    area("key", (-3.0, -3.2, 4.2), 420, 3.0, (1.0, 0.96, 0.9))
    area("fill", (3.6, -2.4, 1.6), 130, 4.0, (1.0, 0.86, 0.78))
    area("rim", (1.6, 3.4, 3.2), 300, 2.0, (1.0, 0.82, 0.62))

world = bpy.data.worlds.new("w"); scene.world = world; world.use_nodes = True
bg = world.node_tree.nodes["Background"]; bg.inputs["Color"].default_value = hex2lin("#F5F5F7" if STUDIO else PEACH); bg.inputs["Strength"].default_value = 0.45 if STUDIO else 0.35

# ---------------------------------------------------------------- render
scene.render.engine = "CYCLES"; scene.cycles.device = "CPU"; scene.cycles.samples = SAMPLES
scene.cycles.use_denoising = True
scene.render.film_transparent = True
scene.view_settings.view_transform = "AgX"; scene.view_settings.look = "AgX - Punchy"
r = scene.render; r.resolution_x = RES; r.resolution_y = int(RES * aspect[1] / aspect[0]); r.resolution_percentage = 100
r.image_settings.file_format = "PNG"; r.image_settings.color_mode = "RGBA"; r.filepath = OUT
if BLEND: bpy.ops.wm.save_as_mainfile(filepath=BLEND)
bpy.ops.render.render(write_still=True)
print("wrote", OUT)
