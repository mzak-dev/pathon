"""Exports the Blender "classroom" scene (_mainScene) to glTF 2.0 for pathon tests.

Run (Blender 5.x; 4.5 has the same exporter but the install here lacks blender.exe):

    blender -b classroom.blend --python tools/scenes/export_classroom.py -- <out_dir> \
        [--variant full|lite|all] [--max-texture-size 2048] [--lite-texture-size 1024] \
        [--env-hdr <path to Garage.hdr>] [--no-extra-cameras]

Output (see docs/scenes.md): <out_dir>/classroom.glb, classroom-lite.glb, env/Garage.hdr and
<name>.scene.json (manifest: cameras, lights with unit conversion, losses, statistics).
Nothing is written back to the .blend file or to the source textures: everything below edits the
in-memory session only.
"""

import argparse
import json
import math
import os
import shutil
import struct
import sys
import time
from collections import Counter
from dataclasses import dataclass, field, replace
from typing import Optional

import bpy
import mathutils

MAIN_SCENE = "_mainScene"
PORTAL_MATERIAL = "dayLight_portal"
# Small props dropped from the "lite" variant (instanced collections living in the .blend assets).
LITE_DROPPED_COLLECTIONS = {"pencil", "plasticGoldPen", "rotring", "largeBox", "crinkledPaper"}
WATTS_TO_LUMENS = 683.0  # same constant as the Blender glTF exporter (KHR_lights_punctual, SPEC mode)
DEFAULT_ROUGHNESS_WHEN_TEXTURED = 0.4
DIFFUSE_ROUGHNESS = 0.9


def parse_args():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    parser = argparse.ArgumentParser(prog="export_classroom.py")
    parser.add_argument("out_dir")
    parser.add_argument("--variant", choices=["full", "lite", "all"], default="all")
    parser.add_argument("--max-texture-size", type=int, default=2048)
    parser.add_argument("--lite-texture-size", type=int, default=1024)
    parser.add_argument("--env-hdr", default=None)
    parser.add_argument("--no-extra-cameras", action="store_true")
    return parser.parse_args(argv)


# --------------------------------------------------------------------------------------------
# Colour helpers
# --------------------------------------------------------------------------------------------

def blackbody_rgb(kelvin):
    """Planckian locus (Kim et al. cubic spline) -> linear sRGB, normalised to max channel 1."""
    t = min(max(kelvin, 1667.0), 25000.0)
    if t <= 4000.0:
        x = -0.2661239e9 / t**3 - 0.2343589e6 / t**2 + 0.8776956e3 / t + 0.179910
    else:
        x = -3.0258469e9 / t**3 + 2.1070379e6 / t**2 + 0.2226347e3 / t + 0.240390
    if t <= 2222.0:
        y = -1.1063814 * x**3 - 1.34811020 * x**2 + 2.18555832 * x - 0.20219683
    elif t <= 4000.0:
        y = -0.9549476 * x**3 - 1.37418593 * x**2 + 2.09137015 * x - 0.16748867
    else:
        y = 3.0817580 * x**3 - 5.87338670 * x**2 + 3.75112997 * x - 0.37001483
    big_x, big_z = x / y, (1.0 - x - y) / y
    r = 3.2404542 * big_x - 1.5371385 + -0.4985314 * big_z
    g = -0.9692660 * big_x + 1.8760108 + 0.0415560 * big_z
    b = 0.0556434 * big_x - 0.2040259 + 1.0572252 * big_z
    r, g, b = max(r, 0.0), max(g, 0.0), max(b, 0.0)
    peak = max(r, g, b) or 1.0
    return (r / peak, g / peak, b / peak)


# --------------------------------------------------------------------------------------------
# Shader-graph analysis: Cycles node tree -> approximate glTF metallic-roughness parameters
# --------------------------------------------------------------------------------------------

@dataclass
class Up:
    """Result of following a socket upstream: either a node output or a constant socket."""
    node: Optional[bpy.types.Node]
    out: Optional[bpy.types.NodeSocket]
    ctx: tuple
    sock: bpy.types.NodeSocket


def upstream(sock, ctx=()):
    while sock.is_linked:
        link = sock.links[0]
        node = link.from_node
        if node.bl_idname == "NodeReroute":
            sock = node.inputs[0]
        elif node.bl_idname == "NodeGroupInput":
            if not ctx:
                return Up(None, None, ctx, sock)
            parent = ctx[-1]
            sock = parent.inputs.get(link.from_socket.name) or sock
            ctx = ctx[:-1]
            if sock is link.from_socket:
                return Up(None, None, ctx, sock)
        elif node.bl_idname == "ShaderNodeGroup" and node.node_tree:
            group_out = next((n for n in node.node_tree.nodes
                              if n.bl_idname == "NodeGroupOutput" and n.is_active_output), None)
            target = group_out.inputs.get(link.from_socket.name) if group_out else None
            if target is None:
                return Up(None, None, ctx, sock)
            sock, ctx = target, ctx + (node,)
        else:
            return Up(node, link.from_socket, ctx, sock)
    return Up(None, None, ctx, sock)


def const_value(sock):
    value = getattr(sock, "default_value", None)
    return float(value) if isinstance(value, (int, float)) else None


def eval_float(sock, ctx):
    """Constant float behind the socket, or None when it is driven by a procedural/texture input."""
    up = upstream(sock, ctx)
    if up.node is None:
        return const_value(up.sock)
    if up.node.bl_idname == "ShaderNodeValue":
        return float(up.node.outputs[0].default_value)
    return None


@dataclass
class ColorSrc:
    rgb: tuple = (0.8, 0.8, 0.8)      # constant colour (multiplies the texture when `tex` is set)
    tex: Optional[bpy.types.Node] = None
    tint: tuple = (1.0, 1.0, 1.0)
    occlusion_tex: Optional[bpy.types.Node] = None


def is_usable_image_node(node):
    image = getattr(node, "image", None)
    return image is not None and image.size[0] > 0 and image.size[1] > 0


def is_occlusion_image(node):
    return is_usable_image_node(node) and node.image.name.lower().split(".")[0].endswith("_ao")


PASS_THROUGH_COLOR_NODES = {
    "ShaderNodeHueSaturation": "Color", "ShaderNodeRGBCurve": "Color", "ShaderNodeInvert": "Color",
    "ShaderNodeGamma": "Color", "ShaderNodeBrightContrast": "Color",
}


def eval_color(sock, ctx, losses):
    up = upstream(sock, ctx)
    if up.node is None:
        value = up.sock.default_value
        return ColorSrc(rgb=tuple(value[:3]))
    node = up.node
    kind = node.bl_idname
    if kind == "ShaderNodeTexImage":
        if is_usable_image_node(node):
            if is_occlusion_image(node):
                return ColorSrc(rgb=(1.0, 1.0, 1.0), occlusion_tex=node)
            return ColorSrc(rgb=(1.0, 1.0, 1.0), tex=node)
        losses.append("missing_image")
        return ColorSrc()
    if kind == "ShaderNodeRGB":
        return ColorSrc(rgb=tuple(node.outputs[0].default_value[:3]))
    if kind == "ShaderNodeBlackbody":
        return ColorSrc(rgb=blackbody_rgb(eval_float(node.inputs["Temperature"], up.ctx) or 6500.0))
    if kind in PASS_THROUGH_COLOR_NODES:
        inner = eval_color(node.inputs[PASS_THROUGH_COLOR_NODES[kind]], up.ctx, losses)
        losses.append("colour_adjust_node_ignored:" + kind.replace("ShaderNode", ""))
        if kind == "ShaderNodeHueSaturation":
            value = eval_float(node.inputs["Value"], up.ctx) or 1.0
            inner.tint = tuple(c * value for c in inner.tint)
        return inner
    if kind == "ShaderNodeValToRGB":
        factor = upstream(node.inputs["Fac"], up.ctx)
        if factor.node is not None and factor.node.bl_idname == "ShaderNodeTexImage" and is_usable_image_node(factor.node):
            losses.append("colour_ramp_replaced_by_source_texture")
            return ColorSrc(rgb=(1.0, 1.0, 1.0), tex=factor.node)
        losses.append("procedural_colour_constant")
        return ColorSrc(rgb=tuple(node.color_ramp.evaluate(0.5)[:3]))
    if kind == "ShaderNodeMix" and node.data_type == "RGBA":
        return eval_color_mix(node, up.ctx, losses)
    losses.append("procedural_colour_constant:" + kind.replace("ShaderNode", ""))
    return ColorSrc(rgb=(0.5, 0.5, 0.5))


def eval_color_mix(node, ctx, losses):
    a_sock = next(s for s in node.inputs if s.identifier == "A_Color")
    b_sock = next(s for s in node.inputs if s.identifier == "B_Color")
    a, b = eval_color(a_sock, ctx, losses), eval_color(b_sock, ctx, losses)
    factor = eval_float(node.inputs["Factor"], ctx)
    weight_b = 0.5 if factor is None else min(max(factor, 0.0), 1.0)
    if upstream(a_sock, ctx).node is not None and upstream(a_sock, ctx).node.bl_idname == "ShaderNodeAttribute":
        losses.append("attribute_colour_ignored")
        return b
    if upstream(b_sock, ctx).node is not None and upstream(b_sock, ctx).node.bl_idname == "ShaderNodeAttribute":
        losses.append("attribute_colour_ignored")
        return a
    occlusion = a.occlusion_tex or b.occlusion_tex
    a_has, b_has = a.tex is not None, b.tex is not None
    if a_has and b_has:
        chosen = b if weight_b > 0.5 else a
        losses.append("texture_mix_replaced_by_dominant_texture")
    elif a_has or b_has:
        chosen, other = (a, b) if a_has else (b, a)
        if node.blend_type in ("MULTIPLY", "OVERLAY", "SOFT_LIGHT", "MIX") and other.tex is None and other.occlusion_tex is None:
            if node.blend_type == "MULTIPLY" or weight_b not in (0.0, 1.0):
                chosen = replace(chosen, tint=tuple(t * c for t, c in zip(chosen.tint, other.rgb)))
        losses.append("texture_blend_approximated")
    else:
        chosen = replace(b if weight_b > 0.5 else a)
        chosen.rgb = tuple(x * (1 - weight_b) + y * weight_b for x, y in zip(a.rgb, b.rgb))
    return replace(chosen, occlusion_tex=occlusion)


@dataclass
class Shader:
    kind: str = "diffuse"
    base: ColorSrc = field(default_factory=ColorSrc)
    metallic: float = 0.0
    roughness: float = DIFFUSE_ROUGHNESS
    emission: Optional[ColorSrc] = None
    emission_strength: float = 0.0
    alpha: float = 1.0
    alpha_tex: Optional[bpy.types.Node] = None
    transmission: float = 0.0
    ior: float = 1.5


def eval_roughness(sock, ctx, losses):
    value = eval_float(sock, ctx)
    if value is None:
        losses.append("roughness_texture_replaced_by_constant")
        return DEFAULT_ROUGHNESS_WHEN_TEXTURED
    return value


def factor_texture(sock, ctx):
    """First usable image node feeding a mix factor (used as alpha mask)."""
    up = upstream(sock, ctx)
    seen, stack = set(), [up]
    while stack:
        current = stack.pop()
        node = current.node
        if node is None or node in seen:
            continue
        seen.add(node)
        if node.bl_idname == "ShaderNodeTexImage" and is_usable_image_node(node):
            return node
        stack.extend(upstream(i, current.ctx) for i in node.inputs if i.is_linked)
    return None


def eval_shader(sock, ctx, losses):
    up = upstream(sock, ctx)
    if up.node is None:
        return Shader(kind="none", base=ColorSrc(rgb=(0.8, 0.8, 0.8)))
    node, c = up.node, up.ctx
    kind = node.bl_idname
    if node.inputs.get("Normal") is not None and node.inputs["Normal"].is_linked:
        losses.append("bump_or_normal_ignored")
    if kind in ("ShaderNodeBsdfDiffuse", "ShaderNodeBsdfTranslucent"):
        return Shader("diffuse", base=eval_color(node.inputs["Color"], c, losses))
    if kind in ("ShaderNodeBsdfAnisotropic", "ShaderNodeBsdfGlossy"):
        return Shader("glossy", base=eval_color(node.inputs["Color"], c, losses), metallic=1.0,
                      roughness=eval_roughness(node.inputs["Roughness"], c, losses))
    if kind == "ShaderNodeBsdfGlass":
        ior = eval_float(node.inputs["IOR"], c) or 1.5
        return Shader("glass", base=eval_color(node.inputs["Color"], c, losses), transmission=1.0,
                      roughness=eval_roughness(node.inputs["Roughness"], c, losses), ior=ior)
    if kind == "ShaderNodeEmission":
        strength = eval_float(node.inputs["Strength"], c)
        if strength is None:
            losses.append("emission_strength_constant")
            strength = 1.0
        return Shader("emission", base=ColorSrc(rgb=(0.0, 0.0, 0.0)), roughness=1.0,
                      emission=eval_color(node.inputs["Color"], c, losses), emission_strength=strength)
    if kind == "ShaderNodeBsdfTransparent":
        return Shader("transparent", alpha=0.0)
    if kind == "ShaderNodeBsdfPrincipled":
        return eval_principled(node, c, losses)
    if kind == "ShaderNodeMixShader":
        return eval_mix_shader(node, c, losses)
    if kind == "ShaderNodeAddShader":
        first, second = eval_shader(node.inputs[0], c, losses), eval_shader(node.inputs[1], c, losses)
        return mix_shaders(first, second, 0.5, None, None)
    losses.append("unsupported_shader:" + kind.replace("ShaderNode", ""))
    return Shader("diffuse")


def eval_principled(node, ctx, losses):
    shader = Shader("principled", base=eval_color(node.inputs["Base Color"], ctx, losses))
    shader.metallic = eval_float(node.inputs["Metallic"], ctx) or 0.0
    shader.roughness = eval_roughness(node.inputs["Roughness"], ctx, losses)
    shader.alpha = eval_float(node.inputs["Alpha"], ctx) or 1.0
    shader.transmission = eval_float(node.inputs["Transmission Weight"], ctx) or 0.0
    shader.ior = eval_float(node.inputs["IOR"], ctx) or 1.5
    shader.emission = eval_color(node.inputs["Emission Color"], ctx, losses)
    shader.emission_strength = eval_float(node.inputs["Emission Strength"], ctx) or 0.0
    return shader


def eval_mix_shader(node, ctx, losses):
    first, second = eval_shader(node.inputs[1], ctx, losses), eval_shader(node.inputs[2], ctx, losses)
    factor = eval_float(node.inputs["Fac"], ctx)
    mask_tex = None
    if factor is None:
        mask_tex = factor_texture(node.inputs["Fac"], ctx)
        losses.append("shader_mix_driven_by_texture")
    return mix_shaders(first, second, factor, mask_tex, losses)


def lerp(a, b, t):
    return a * (1 - t) + b * t


def mix_shaders(first, second, factor, mask_tex, losses):
    w2 = 0.5 if factor is None else min(max(factor, 0.0), 1.0)
    w1 = 1.0 - w2
    if "transparent" in (first.kind, second.kind) and first.kind != second.kind:
        solid, w_solid = (second, w2) if first.kind == "transparent" else (first, w1)
        result = replace(solid)
        if factor is None:
            result.alpha, result.alpha_tex = 1.0, mask_tex
        else:
            result.alpha = w_solid
        return result
    dominant = second if w2 > 0.5 else first
    result = replace(dominant)
    if {first.kind, second.kind} == {"diffuse", "glossy"}:
        glossy_weight = w2 if second.kind == "glossy" else w1
        diffuse, glossy = (first, second) if first.kind == "diffuse" else (second, first)
        result = replace(diffuse)
        result.metallic = glossy_weight if glossy_weight >= 0.5 else 0.0
        if glossy_weight >= 0.25:
            result.roughness = glossy.roughness
        else:
            result.roughness = lerp(diffuse.roughness, glossy.roughness, glossy_weight / 0.25)
        if glossy_weight >= 0.5:
            result.base = glossy.base if glossy.base.tex else diffuse.base
        return result
    result.metallic = lerp(first.metallic, second.metallic, w2)
    result.roughness = lerp(first.roughness, second.roughness, w2)
    result.transmission = lerp(first.transmission, second.transmission, w2)
    result.alpha = lerp(first.alpha, second.alpha, w2)
    if first.emission is not None or second.emission is not None:
        source = second if (second.emission is not None and (first.emission is None or w2 >= 0.5)) else first
        result.emission = source.emission
        result.emission_strength = lerp(first.emission_strength, second.emission_strength, w2)
    return result


def find_material_output(tree):
    outputs = [n for n in tree.nodes if n.bl_idname == "ShaderNodeOutputMaterial"]
    for wanted in ("CYCLES", "ALL"):
        for node in outputs:
            if node.target == wanted and node.inputs["Surface"].is_linked:
                return node
    return next((n for n in outputs if n.inputs["Surface"].is_linked), None)


def mix_node_identifiers(node):
    by_id = {s.identifier: s for s in node.inputs}
    return by_id["A_Color"], by_id["B_Color"], next(s for s in node.outputs if s.identifier == "Result_Color")


def link_color(tree, src: ColorSrc, target_socket):
    """Wire a ColorSrc to a Principled colour input (texture times constant via a Multiply mix node)."""
    tint = tuple(r * t for r, t in zip(src.rgb, src.tint)) if src.tex else tuple(r * t for r, t in zip(src.rgb, src.tint))
    if src.tex is None:
        target_socket.default_value = (*tint, 1.0)
        return
    if all(abs(c - 1.0) < 1e-3 for c in tint):
        tree.links.new(src.tex.outputs["Color"], target_socket)
        return
    mix = tree.nodes.new("ShaderNodeMix")
    mix.data_type, mix.blend_type = "RGBA", "MULTIPLY"
    mix.inputs["Factor"].default_value = 1.0
    a_sock, b_sock, result = mix_node_identifiers(mix)
    tree.links.new(src.tex.outputs["Color"], a_sock)
    b_sock.default_value = (*tint, 1.0)
    tree.links.new(result, target_socket)


def occlusion_group():
    name = "glTF Material Output"
    group = bpy.data.node_groups.get(name)
    if group is None:
        group = bpy.data.node_groups.new(name, "ShaderNodeTree")
        group.interface.new_socket("Occlusion", in_out="INPUT", socket_type="NodeSocketFloat")
    return group


def build_principled(material, shader: Shader):
    tree = material.node_tree
    old_outputs = [n for n in tree.nodes if n.bl_idname == "ShaderNodeOutputMaterial"]
    bsdf = tree.nodes.new("ShaderNodeBsdfPrincipled")
    output = tree.nodes.new("ShaderNodeOutputMaterial")
    for node in old_outputs:
        tree.nodes.remove(node)
    output.is_active_output, output.target = True, "ALL"
    tree.links.new(bsdf.outputs["BSDF"], output.inputs["Surface"])

    link_color(tree, shader.base, bsdf.inputs["Base Color"])
    bsdf.inputs["Metallic"].default_value = min(max(shader.metallic, 0.0), 1.0)
    bsdf.inputs["Roughness"].default_value = min(max(shader.roughness, 0.0), 1.0)
    bsdf.inputs["IOR"].default_value = shader.ior
    bsdf.inputs["Transmission Weight"].default_value = shader.transmission
    if shader.emission is not None and shader.emission_strength > 0:
        link_color(tree, shader.emission, bsdf.inputs["Emission Color"])
        bsdf.inputs["Emission Strength"].default_value = shader.emission_strength
    if shader.alpha_tex is not None:
        tree.links.new(shader.alpha_tex.outputs["Alpha"], bsdf.inputs["Alpha"])
    else:
        bsdf.inputs["Alpha"].default_value = shader.alpha
    if shader.base.occlusion_tex is not None:
        group_node = tree.nodes.new("ShaderNodeGroup")
        group_node.node_tree = occlusion_group()
        tree.links.new(shader.base.occlusion_tex.outputs["Color"], group_node.inputs["Occlusion"])


def convert_materials():
    """Replaces every material's output chain with an equivalent Principled BSDF; returns a report."""
    report = {}
    for material in bpy.data.materials:
        if material.node_tree is None or not material.users:
            continue
        output = find_material_output(material.node_tree)
        if output is None:
            continue
        losses = []
        if output.inputs["Surface"].links[0].from_node.bl_idname == "ShaderNodeBsdfPrincipled":
            report[material.name] = {"kind": "principled", "losses": []}
            continue
        if output.inputs["Displacement"].is_linked:
            losses.append("displacement_ignored")
        shader = eval_shader(output.inputs["Surface"], (), losses)
        build_principled(material, shader)
        report[material.name] = {
            "kind": shader.kind, "metallic": round(shader.metallic, 3), "roughness": round(shader.roughness, 3),
            "alpha": round(shader.alpha, 3) if shader.alpha_tex is None else "texture",
            "transmission": shader.transmission, "emission_strength": shader.emission_strength,
            "base_texture": shader.base.tex.image.name if shader.base.tex else None,
            "occlusion_texture": shader.base.occlusion_tex.image.name if shader.base.occlusion_tex else None,
            "losses": sorted(set(losses)),
        }
    return report


# --------------------------------------------------------------------------------------------
# Lights: Cycles light nodes + Blender watts -> values the glTF exporter converts to lux/candela
# --------------------------------------------------------------------------------------------

def light_emission_node(light):
    if not light.node_tree:
        return None
    return next((n for n in light.node_tree.nodes if n.bl_idname == "ShaderNodeEmission"), None)


def lights_used_by(scene):
    used = set()
    for obj in scene.objects:
        objects = obj.instance_collection.all_objects if obj.instance_type == "COLLECTION" and obj.instance_collection else [obj]
        used.update(o.data for o in objects if o.type == "LIGHT")
    return used


def convert_lights(scene):
    """Bakes the Emission node (strength, falloff strength, blackbody colour) into energy/colour.

    The exporter would otherwise ignore lamp energy for sun lights and misread node strength, and it
    drops AREA lights; those become hemisphere-wide spot lights carrying the original in extras.
    """
    report = []
    for light in sorted(lights_used_by(scene), key=lambda l: l.name):
        strength_mult, color = 1.0, tuple(light.color)
        emission = light_emission_node(light)
        if emission is not None:
            strength = upstream(emission.inputs["Strength"], ())
            if strength.node is not None and strength.node.bl_idname == "ShaderNodeLightFalloff":
                strength_mult = float(strength.node.inputs["Strength"].default_value)
            elif strength.node is None:
                strength_mult = float(strength.sock.default_value)
            tint = eval_color(emission.inputs["Color"], (), [])
            color = tuple(c * t for c, t in zip(color, tint.rgb))
        entry = {"name": light.name, "blender_type": light.type, "blender_energy": light.energy,
                 "emission_multiplier": strength_mult, "color": [round(c, 4) for c in color]}
        effective = light.energy * strength_mult
        if light.type == "AREA":
            light["pathon_area"] = json.dumps({
                "shape": light.shape, "size_x": light.size, "size_y": light.size_y if light.shape in ("RECTANGLE", "ELLIPSE") else light.size,
                "power_w": effective, "note": "one-sided Lambertian emitter, exported as hemisphere spot"})
            light.type = "SPOT"
            light = bpy.data.lights[entry["name"]]
            light.spot_size, light.spot_blend = math.pi - 0.002, 0.15  # outer cone must stay below pi/2 in float32
            effective *= 2.0  # exporter divides by 4*pi; a hemisphere spot keeping total flux needs P/(2*pi)
            entry["exported_as"] = "spot (area light approximation, see pathon_area extra)"
        light.energy = effective
        light.color = color
        light.use_temperature = False
        if light.node_tree:
            light.node_tree.nodes.clear()  # the exporter reads the Emission node even when use_nodes is off
        entry["effective_energy"] = effective
        report.append(entry)
    return report


# --------------------------------------------------------------------------------------------
# Scene preparation
# --------------------------------------------------------------------------------------------

def keep_only_main_scene():
    main = bpy.data.scenes[MAIN_SCENE]
    for scene in list(bpy.data.scenes):
        if scene is not main:
            bpy.data.scenes.remove(scene)
    return main


def bake_object_material_links():
    """The glTF exporter reads mesh-data materials and ignores slots linked to the object; bake them in."""
    fixed = 0
    for obj in bpy.data.objects:
        if obj.type != "MESH" or not any(slot.link == "OBJECT" for slot in obj.material_slots):
            continue
        mesh = obj.data
        if mesh.users > 1:
            mesh = mesh.copy()
            obj.data = mesh
        for index, slot in enumerate(obj.material_slots):
            if slot.link == "OBJECT":
                material = slot.material
                slot.link = "DATA"
                mesh.materials[index] = material
        fixed += 1
    return fixed


def drop_portal_objects(scene):
    names = []
    for obj in list(scene.objects):
        if any(slot.material and slot.material.name.startswith(PORTAL_MATERIAL) for slot in obj.material_slots):
            names.append(obj.name)
            bpy.data.objects.remove(obj)
    return names


def drop_lite_objects(scene):
    names = []
    for obj in list(scene.objects):
        if obj.instance_type == "COLLECTION" and obj.instance_collection and obj.instance_collection.name in LITE_DROPPED_COLLECTIONS:
            names.append(obj.name)
            bpy.data.objects.remove(obj)
    return names


def downscale_images(max_size):
    count = 0
    for image in bpy.data.images:
        if image.source not in ("FILE", "GENERATED") or not image.users:
            continue
        width, height = image.size
        biggest = max(width, height)
        if biggest > max_size:
            ratio = max_size / biggest
            image.scale(max(1, round(width * ratio)), max(1, round(height * ratio)))
            count += 1
    return count


def scene_bounds(scene):
    depsgraph = bpy.context.evaluated_depsgraph_get()
    low, high = mathutils.Vector((1e9,) * 3), mathutils.Vector((-1e9,) * 3)
    for inst in depsgraph.object_instances:
        if inst.object.type != "MESH":
            continue
        for corner in inst.object.bound_box:
            world = inst.matrix_world @ mathutils.Vector(corner)
            low = mathutils.Vector(map(min, low, world))
            high = mathutils.Vector(map(max, high, world))
    return low, high


def add_extra_cameras(scene):
    main = scene.camera
    if main is None:
        return []
    created = []

    def new_camera(name, location, target, lens):
        data = bpy.data.cameras.new(name)
        data.lens, data.sensor_width, data.sensor_fit = lens, main.data.sensor_width, main.data.sensor_fit
        data.clip_start, data.clip_end = main.data.clip_start, main.data.clip_end
        obj = bpy.data.objects.new(name, data)
        scene.collection.objects.link(obj)
        direction = mathutils.Vector(target) - mathutils.Vector(location)
        obj.rotation_euler = direction.to_track_quat("-Z", "Y").to_euler()
        obj.location = location
        created.append(name)

    main_loc = main.matrix_world.translation
    new_camera("pathon_wide", main_loc, main_loc + main.matrix_world.to_3x3() @ mathutils.Vector((0, 0, -1)), 14.0)
    board = bpy.data.objects.get("blackBoard_light")
    if board is not None:
        board_loc = board.matrix_world.translation
        new_camera("pathon_reverse", (board_loc.x, board_loc.y - 0.8, 1.5), (main_loc.x, main_loc.y, 1.2), 20.0)
    return created


def camera_report(scene):
    cameras = []
    for obj in scene.objects:
        if obj.type != "CAMERA":
            continue
        data, loc = obj.data, obj.matrix_world.translation
        width, height = scene.render.resolution_x, scene.render.resolution_y
        sensor = data.sensor_width
        horizontal = 2 * math.atan(sensor / (2 * data.lens))
        yfov = 2 * math.atan(math.tan(horizontal / 2) * height / width)
        cameras.append({
            "name": obj.name, "main": obj is scene.camera, "lens_mm": data.lens, "sensor_width_mm": sensor,
            "yfov_rad": yfov, "aspect": width / height, "znear": data.clip_start, "zfar": data.clip_end,
            "position_gltf": [loc.x, loc.z, -loc.y], "position_blender_zup": [loc.x, loc.y, loc.z],
            "resolution": [width, height],
        })
    return cameras


# --------------------------------------------------------------------------------------------
# Export and statistics
# --------------------------------------------------------------------------------------------

def export_glb(path, image_format):
    bpy.ops.export_scene.gltf(
        filepath=path, export_format="GLB", use_active_scene=True, export_apply=True, export_skins=False,
        export_animations=False, export_cameras=True, export_lights=True, export_extras=True, export_yup=True,
        export_image_format=image_format, export_jpeg_quality=85, export_import_convert_lighting_mode="SPEC",
        export_materials="EXPORT", export_gpu_instances=False)


def read_glb_json(path):
    with open(path, "rb") as handle:
        magic, version, _length = struct.unpack("<4sII", handle.read(12))
        chunk_length, chunk_type = struct.unpack("<II", handle.read(8))
        assert magic == b"glTF" and chunk_type == 0x4E4F534A
        return json.loads(handle.read(chunk_length))


def glb_stats(path):
    doc = read_glb_json(path)
    primitives = [p for m in doc.get("meshes", []) for p in m["primitives"]]
    triangles = 0
    for prim in primitives:
        if "indices" in prim:
            triangles += doc["accessors"][prim["indices"]]["count"] // 3
    light_nodes = sum(1 for n in doc.get("nodes", []) if "KHR_lights_punctual" in n.get("extensions", {}))
    lights = doc.get("extensions", {}).get("KHR_lights_punctual", {}).get("lights", [])
    return {
        "file_bytes": os.path.getsize(path), "nodes": len(doc.get("nodes", [])), "meshes": len(doc.get("meshes", [])),
        "primitives": len(primitives), "triangles": triangles, "materials": len(doc.get("materials", [])),
        "textures": len(doc.get("textures", [])), "images": len(doc.get("images", [])),
        "cameras": len(doc.get("cameras", [])), "light_definitions": len(lights), "light_nodes": light_nodes,
        "extensions_used": doc.get("extensionsUsed", []),
        "image_mime": dict(Counter(i.get("mimeType") for i in doc.get("images", []))),
        "lights": [{"name": l.get("name"), "type": l["type"], "intensity": l.get("intensity"),
                    "color": l.get("color")} for l in lights],
    }


def export_variant(scene, name, out_dir, texture_limit, image_format, manifest_base):
    scaled = downscale_images(texture_limit)
    path = os.path.join(out_dir, name + ".glb")
    started = time.time()
    export_glb(path, image_format)
    elapsed = time.time() - started
    stats = glb_stats(path)
    manifest = dict(manifest_base, variant=name, image_format=image_format, export_seconds=round(elapsed, 1), max_texture_size=texture_limit,
                    textures_downscaled=scaled, stats=stats, cameras=camera_report(scene))
    with open(os.path.join(out_dir, name + ".scene.json"), "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, indent=2)
    print("PATHON_EXPORT_SUMMARY " + json.dumps({k: manifest[k] for k in ("variant", "export_seconds", "stats")}))
    return manifest


def main():
    args = parse_args()
    started = time.time()
    os.makedirs(args.out_dir, exist_ok=True)
    blend_dir = os.path.dirname(bpy.data.filepath)
    scene = keep_only_main_scene()
    object_material_links = bake_object_material_links()

    portal_objects = drop_portal_objects(scene)
    material_report = convert_materials()
    light_report = convert_lights(scene)
    extra_cameras = [] if args.no_extra_cameras else add_extra_cameras(scene)
    bpy.context.view_layer.update()

    env_source = args.env_hdr or os.path.join(blend_dir, "assets", "__ENV", "Garage", "Garage.hdr")
    env_target = None
    if os.path.isfile(env_source):
        os.makedirs(os.path.join(args.out_dir, "env"), exist_ok=True)
        env_target = os.path.join(args.out_dir, "env", "Garage.hdr")
        shutil.copyfile(env_source, env_target)

    loss_counter = Counter(tag for entry in material_report.values() for tag in entry["losses"])
    manifest_base = {
        "generator": "tools/scenes/export_classroom.py", "blender": bpy.app.version_string,
        "source": "classroom.blend (Christophe Seux), scene " + MAIN_SCENE,
        "units": "meters, Y up (glTF), lights in lux (sun) and candela (point/spot), KHR_lights_punctual",
        "lighting_conversion": {
            "sun": "lux = W/m2 * emission_strength * 683", "point_spot": "candela = W * emission_strength / (4*pi) * 683",
            "area": "approximated by a hemisphere spot (outer cone 90 deg), candela = W/(2*pi) * 683; original in extras.pathon_area",
            "exposure_hint": "Blender renders at exposure 0 (Standard); to match, pathon exposure ~ 1/683",
        },
        "environment": {"hdr": os.path.relpath(env_target, args.out_dir).replace("\\", "/") if env_target else None,
                        "projection": "equirectangular, linear Radiance HDR", "blender_world": "black, strength 0 (no IBL in the original render)"},
        "dropped_portal_objects": portal_objects, "objects_with_object_linked_materials_baked": object_material_links, "extra_cameras": extra_cameras,
        "material_conversion": material_report, "material_loss_summary": dict(loss_counter),
        "lights_conversion": light_report,
    }

    manifests = []
    if args.variant in ("full", "all"):
        manifests.append(export_variant(scene, "classroom", args.out_dir, args.max_texture_size, "AUTO", manifest_base))
    if args.variant in ("lite", "all"):
        dropped = drop_lite_objects(scene)
        manifest_base = dict(manifest_base, lite_dropped_instances=dropped)
        manifests.append(export_variant(scene, "classroom-lite", args.out_dir, args.lite_texture_size, "JPEG", manifest_base))
    print("PATHON_EXPORT_TOTAL_SECONDS %.1f" % (time.time() - started))


main()
