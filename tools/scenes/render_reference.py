"""Renders the reference images of the classroom scene in Blender Cycles (pattern B of docs/porownanie-obrazu.md).

The scene is rebuilt from the glTF file pathon itself loads, so only the renderer differs, not the content:

    blender -b --factory-startup --python tools/scenes/render_reference.py -- <out_dir> \
        [--glb test-assets/classroom/classroom.glb] [--cameras renderCam,pathon_wide,pathon_reverse] \
        [--lighting lights-only|ibl|both] [--samples 512] [--denoise off|on|both] [--resolution 960x540] \
        [--area-lights real|spot] [--device auto|hip|optix|cuda|cpu] [--no-aovs] [--calibrate-exposure]

Output in <out_dir>/<lighting>/: <camera>.exr (multilayer, linear Rec.709, float32, no denoising, with AOV passes),
<camera>.png (the same image as sRGB, Standard view transform), <camera>_denoised.exr/.png (OpenImageDenoise), and in
<out_dir>/reference.json every setting needed to repeat the run. Nothing is written to the source files.
"""

import argparse
import hashlib
import json
import math
import os
import sys
import time
from pathlib import Path

import bpy

LUMENS_PER_WATT = 683.0  # the glTF exporter and importer constant (KHR_lights_punctual, SPEC mode)
HDR_ROTATION_Z = -math.pi / 2  # the pathon IBL puts the middle of the picture on glTF -Z, Cycles puts it on +X
DEVICE_ORDER = ("HIP", "OPTIX", "CUDA", "ONEAPI")
DEFAULT_CAMERAS = ("renderCam", "pathon_wide", "pathon_reverse")
LIGHT_PATH_LIMITS = {"max_bounces": 16, "diffuse_bounces": 8, "glossy_bounces": 8, "transmission_bounces": 12,
                     "volume_bounces": 0, "transparent_max_bounces": 16}
AOV_PASSES = (
    "z", "normal", "diffuse_direct", "diffuse_indirect", "diffuse_color", "glossy_direct", "glossy_indirect",
    "glossy_color", "transmission_direct", "transmission_indirect", "transmission_color", "emit", "environment",
    "ambient_occlusion", "material_index",
)


def parse_args():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    repo = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(prog="render_reference.py")
    parser.add_argument("out_dir")
    parser.add_argument("--glb", default=str(repo / "test-assets/classroom/classroom.glb"))
    parser.add_argument("--scene-json", default=None, help="default: next to the .glb, <name>.scene.json")
    parser.add_argument("--hdr", default=None, help="default: env/Garage.hdr next to the .glb")
    parser.add_argument("--cameras", default=",".join(DEFAULT_CAMERAS))
    parser.add_argument("--lighting", choices=["lights-only", "ibl", "both"], default="both")
    parser.add_argument("--samples", type=int, default=512)
    parser.add_argument("--denoise", choices=["off", "on", "both"], default="both",
                        help="off: only the raw render; on: only the denoised one; both: two renders per camera")
    parser.add_argument("--resolution", default="960x540")
    parser.add_argument("--area-lights", choices=["real", "spot"], default="real",
                        help="real: rectangles from extras.pathon_area; spot: the hemisphere spots of the .glb")
    parser.add_argument("--device", choices=["auto", "hip", "optix", "cuda", "cpu"], default="auto")
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--clamp-indirect", type=float, default=0.0,
                        help="0 (default) disables the clamp: the Blender default of 10 darkens this scene by about 20%%")
    parser.add_argument("--exposure-ev", type=float, default=0.0, help="only the PNG files; the EXR stays raw")
    parser.add_argument("--ibl-strength", type=float, default=1.0)
    parser.add_argument("--no-aovs", action="store_true")
    parser.add_argument("--calibrate-exposure", action="store_true",
                        help="measure Cycles radiance of a white plane under a sun of known strength first")
    return parser.parse_args(argv)


# --------------------------------------------------------------------------------------------
# Cycles setup
# --------------------------------------------------------------------------------------------

def select_device(preference):
    """Enables the GPU of one backend (the first available in DEVICE_ORDER) or falls back to the CPU."""
    prefs = bpy.context.preferences.addons["cycles"].preferences
    kinds = DEVICE_ORDER if preference == "auto" else (preference.upper(),) if preference != "cpu" else ()
    for kind in kinds:
        prefs.compute_device_type = kind
        prefs.refresh_devices()
        gpus = [d for d in prefs.devices if d.type == kind]
        if not gpus:
            continue
        for device in prefs.devices:
            device.use = device.type == kind
        if hasattr(prefs, "use_hiprt"):
            prefs.use_hiprt = kind == "HIP"
        return {"backend": kind, "names": [d.name for d in gpus], "hardware_raytracing": getattr(prefs, "use_hiprt", False)
                if kind == "HIP" else kind == "OPTIX"}
    prefs.compute_device_type = "NONE"
    return {"backend": "CPU", "names": [d.name for d in prefs.devices if d.type == "CPU"], "hardware_raytracing": False}


def configure_cycles(scene, args, device):
    scene.render.engine = "CYCLES"
    cycles = scene.cycles
    cycles.device = "CPU" if device["backend"] == "CPU" else "GPU"
    cycles.samples = args.samples
    cycles.use_adaptive_sampling = False
    cycles.seed = args.seed
    cycles.use_animated_seed = False
    cycles.use_denoising = False
    cycles.caustics_reflective = False
    cycles.caustics_refractive = False
    cycles.sample_clamp_direct = 0.0
    cycles.sample_clamp_indirect = args.clamp_indirect
    for name, value in LIGHT_PATH_LIMITS.items():
        setattr(cycles, name, value)
    cycles.pixel_filter_type = "BOX"  # a pixel is the plain average of its area, like a box-filtered supersampled frame
    cycles.filter_width = 1.0
    width, height = (int(v) for v in args.resolution.lower().split("x"))
    scene.render.resolution_x, scene.render.resolution_y = width, height
    scene.render.resolution_percentage = 100
    scene.render.film_transparent = False
    scene.view_settings.view_transform = "Standard"
    scene.view_settings.look = "None"
    scene.view_settings.exposure = args.exposure_ev
    scene.view_settings.gamma = 1.0
    scene.display_settings.display_device = "sRGB"


def configure_passes(view_layer, with_aovs):
    view_layer.use_pass_combined = True
    missing = []
    for name in AOV_PASSES:
        attribute = f"use_pass_{name}"
        if hasattr(view_layer, attribute):
            setattr(view_layer, attribute, with_aovs)
        else:
            missing.append(name)
    return missing


def set_denoising(scene, enabled):
    scene.cycles.use_denoising = enabled
    if enabled:
        scene.cycles.denoiser = "OPENIMAGEDENOISE"
        if hasattr(scene.cycles, "denoising_use_gpu"):
            scene.cycles.denoising_use_gpu = True


# --------------------------------------------------------------------------------------------
# Scene
# --------------------------------------------------------------------------------------------

def import_glb(path):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    started = time.time()
    bpy.ops.import_scene.gltf(filepath=str(path))
    return time.time() - started


def number_materials():
    """Material Index pass: id 1.. in name order, so masks can be taken per material without Cryptomatte."""
    materials = sorted(bpy.data.materials, key=lambda m: m.name)
    for index, material in enumerate(materials, start=1):
        material.pass_index = index
    return {material.name: material.pass_index for material in materials}


def replace_hemisphere_spots_with_area_lights(scene):
    """The .glb carries rectangular lights as spots; the real rectangle is in extras.pathon_area (power in Blender W)."""
    replaced = []
    for obj in [o for o in scene.objects if o.type == "LIGHT" and o.data.get("pathon_area")]:
        shape = json.loads(obj.data["pathon_area"])
        light = bpy.data.lights.new(obj.name + "_area", "AREA")
        light.shape = shape["shape"]
        light.size, light.size_y = shape["size_x"], shape["size_y"]
        light.energy = shape["power_w"]
        light.color = obj.data.color
        light.spread = math.pi
        area = bpy.data.objects.new(obj.name + "_area", light)
        scene.collection.objects.link(area)
        area.matrix_world = obj.matrix_world.copy()
        replaced.append({"name": obj.name, "shape": shape["shape"], "size_m": [shape["size_x"], shape["size_y"]],
                         "power_w": shape["power_w"], "spot_energy_w_replaced": obj.data.energy})
        bpy.data.objects.remove(obj)
    return replaced


def describe_lights(scene):
    lights = []
    for obj in sorted((o for o in scene.objects if o.type == "LIGHT"), key=lambda o: o.name):
        entry = {"name": obj.name, "type": obj.data.type, "energy": obj.data.energy, "color": list(obj.data.color),
                 "position": list(obj.matrix_world.translation)}
        if obj.data.type == "SPOT":
            entry["spot_size_rad"] = obj.data.spot_size
        lights.append(entry)
    return lights


def build_world(scene, lighting, hdr_path, strength):
    world = bpy.data.worlds.new("reference_world")
    world.use_nodes = True
    nodes, links = world.node_tree.nodes, world.node_tree.links
    nodes.clear()
    background = nodes.new("ShaderNodeBackground")
    output = nodes.new("ShaderNodeOutputWorld")
    links.new(background.outputs[0], output.inputs[0])
    if lighting == "ibl":
        coordinates, mapping = nodes.new("ShaderNodeTexCoord"), nodes.new("ShaderNodeMapping")
        mapping.vector_type = "POINT"
        mapping.inputs["Rotation"].default_value = (0.0, 0.0, HDR_ROTATION_Z)
        environment = nodes.new("ShaderNodeTexEnvironment")
        environment.image = bpy.data.images.load(str(hdr_path), check_existing=True)
        environment.interpolation = "Linear"
        links.new(coordinates.outputs["Generated"], mapping.inputs["Vector"])
        links.new(mapping.outputs[0], environment.inputs["Vector"])
        links.new(environment.outputs[0], background.inputs["Color"])
        background.inputs["Strength"].default_value = strength
    else:
        background.inputs["Color"].default_value = (0.0, 0.0, 0.0, 1.0)
        background.inputs["Strength"].default_value = 0.0
    scene.world = world


def find_camera(scene, name):
    camera = next((o for o in scene.objects if o.type == "CAMERA" and o.name == name), None)
    if camera is None:
        raise SystemExit(f"camera {name!r} not found; present: {[o.name for o in scene.objects if o.type == 'CAMERA']}")
    return camera


def describe_camera(camera, expected):
    data = camera.data
    return {"name": camera.name, "yfov_rad": data.angle_y, "yfov_expected_rad": expected["yfov_rad"] if expected else None,
            "position_blender_zup": list(camera.matrix_world.translation),
            "position_expected_zup": expected["position_blender_zup"] if expected else None,
            "clip": [data.clip_start, data.clip_end], "sensor_fit": data.sensor_fit}


# --------------------------------------------------------------------------------------------
# Rendering
# --------------------------------------------------------------------------------------------

def render_to_files(scene, stem, multilayer):
    image_settings = scene.render.image_settings
    image_settings.media_type = "MULTI_LAYER_IMAGE" if multilayer else "IMAGE"
    image_settings.file_format = "OPEN_EXR_MULTILAYER" if multilayer else "OPEN_EXR"
    image_settings.color_depth = "32"
    image_settings.exr_codec = "ZIP"
    scene.render.filepath = str(stem) + ".exr"
    started = time.time()
    bpy.ops.render.render(write_still=True)
    seconds = time.time() - started
    image_settings.media_type = "IMAGE"
    image_settings.file_format = "PNG"
    image_settings.color_mode = "RGB"
    image_settings.color_depth = "8"
    bpy.data.images["Render Result"].save_render(str(stem) + ".png", scene=scene)
    return seconds


def render_lighting(scene, args, lighting, cameras, out_dir, hdr_path, with_aovs):
    build_world(scene, lighting, hdr_path, args.ibl_strength)
    directory = out_dir / lighting
    directory.mkdir(parents=True, exist_ok=True)
    runs = []
    for name in cameras:
        scene.camera = find_camera(scene, name)
        if args.denoise in ("off", "both"):
            set_denoising(scene, False)
            configure_passes(scene.view_layers[0], with_aovs)
            seconds = render_to_files(scene, directory / name, multilayer=True)
            runs.append({"lighting": lighting, "camera": name, "denoised": False, "seconds": round(seconds, 1)})
            print(f"RENDERED {lighting}/{name} raw in {seconds:.1f} s", flush=True)
        if args.denoise in ("on", "both"):
            set_denoising(scene, True)
            configure_passes(scene.view_layers[0], False)
            seconds = render_to_files(scene, directory / f"{name}_denoised", multilayer=False)
            runs.append({"lighting": lighting, "camera": name, "denoised": True, "seconds": round(seconds, 1)})
            print(f"RENDERED {lighting}/{name} denoised in {seconds:.1f} s", flush=True)
    return runs


def measure_sun_patch():
    """Cycles radiance of a white diffuse plane under a 15 W/m2 sun; a unit-albedo Lambertian surface gives S/pi."""
    strength = 15.0
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.device, scene.cycles.samples, scene.cycles.use_denoising = "CPU", 16, False
    scene.render.resolution_x = scene.render.resolution_y = 16
    plane = bpy.data.meshes.new("plane")
    plane.from_pydata([(-5, -5, 0), (5, -5, 0), (5, 5, 0), (-5, 5, 0)], [], [(0, 1, 2, 3)])
    material = bpy.data.materials.new("white")
    material.use_nodes = True
    nodes = material.node_tree.nodes
    nodes.clear()
    diffuse, output = nodes.new("ShaderNodeBsdfDiffuse"), nodes.new("ShaderNodeOutputMaterial")
    diffuse.inputs["Color"].default_value = (1, 1, 1, 1)
    material.node_tree.links.new(diffuse.outputs[0], output.inputs[0])
    plane.materials.append(material)
    scene.collection.objects.link(bpy.data.objects.new("plane", plane))
    sun = bpy.data.lights.new("sun", "SUN")
    sun.energy, sun.angle = strength, 0.0
    sun_object = bpy.data.objects.new("sun", sun)
    sun_object.location = (0, 0, 5)
    scene.collection.objects.link(sun_object)
    camera = bpy.data.cameras.new("camera")
    camera.type, camera.ortho_scale = "ORTHO", 0.2
    camera_object = bpy.data.objects.new("camera", camera)
    camera_object.location = (0, 0, 9)
    scene.collection.objects.link(camera_object)
    scene.camera = camera_object
    world = bpy.data.worlds.new("black")
    world.use_nodes = True
    world.node_tree.nodes["Background"].inputs["Strength"].default_value = 0.0
    scene.world = world
    scene.view_settings.view_transform = "Standard"
    scene.render.image_settings.file_format = "OPEN_EXR"
    scene.render.image_settings.color_depth = "32"
    scene.render.filepath = os.path.join(bpy.app.tempdir, "sun_patch.exr")
    bpy.ops.render.render(write_still=True)
    image = bpy.data.images.load(scene.render.filepath)
    image.colorspace_settings.name = "Linear Rec.709"
    pixels = list(image.pixels)
    radiance = pixels[(8 * 16 + 8) * 4]
    return {"sun_w_per_m2": strength, "measured_radiance": radiance, "expected_s_over_pi": strength / math.pi,
            "ratio_measured_to_expected": radiance * math.pi / strength,
            "pathon_exposure_for_equal_radiance": 1.0 / LUMENS_PER_WATT * strength / (radiance * math.pi)}


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    args = parse_args()
    glb = Path(args.glb).resolve()
    scene_json_path = Path(args.scene_json) if args.scene_json else glb.with_suffix(".scene.json")
    hdr_path = Path(args.hdr) if args.hdr else glb.parent / "env" / "Garage.hdr"
    out_dir = Path(args.out_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    cameras = [c for c in args.cameras.split(",") if c]
    lightings = ["lights-only", "ibl"] if args.lighting == "both" else [args.lighting]

    calibration = measure_sun_patch() if args.calibrate_exposure else None
    expected_cameras = {c["name"]: c for c in json.loads(scene_json_path.read_text(encoding="utf-8"))["cameras"]}
    import_seconds = import_glb(glb)
    scene = bpy.context.scene
    device = select_device(args.device)
    configure_cycles(scene, args, device)
    missing_passes = configure_passes(scene.view_layers[0], not args.no_aovs)
    material_ids = number_materials()
    area_lights = replace_hemisphere_spots_with_area_lights(scene) if args.area_lights == "real" else []

    runs = []
    for lighting in lightings:
        runs += render_lighting(scene, args, lighting, cameras, out_dir, hdr_path, not args.no_aovs)

    report = {
        "blender": bpy.app.version_string, "device": device, "glb": str(glb), "glb_sha256": sha256_of(glb),
        "import_seconds": round(import_seconds, 1), "hdr": str(hdr_path), "hdr_rotation_z_deg": math.degrees(HDR_ROTATION_Z),
        "ibl_strength": args.ibl_strength, "samples": args.samples, "adaptive_sampling": False, "seed": args.seed,
        "light_paths": LIGHT_PATH_LIMITS, "caustics": False, "clamp_indirect": args.clamp_indirect,
        "pixel_filter": "BOX 1.0 px", "resolution": args.resolution, "view_transform": "Standard", "look": "None",
        "exposure_ev_for_png": args.exposure_ev, "denoiser": "OpenImageDenoise (denoised files only)",
        "pathon_exposure_for_equal_radiance": 1.0 / LUMENS_PER_WATT,
        "area_lights": args.area_lights, "area_lights_replaced": area_lights, "lights": describe_lights(scene),
        "cameras": [describe_camera(find_camera(scene, n), expected_cameras.get(n)) for n in cameras],
        "material_ids": material_ids, "passes_missing_in_this_blender": missing_passes, "calibration": calibration,
        "runs": runs,
    }
    (out_dir / "reference.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    print("REPORT", out_dir / "reference.json", flush=True)


main()
