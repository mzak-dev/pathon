// Depth prepass: depth (attachment), normals, motion and material id, plus roughness and albedo when a consumer asks.
// Masked materials discard here, so the colour pass sees the same depth.
import package::material::standard::{MaterialData, load_surface, passes_alpha_test};
import package::material::surface::SurfaceInput;
import package::pbr::common::{Frame, Instance, motion_vector, oct_encode, previous_world_position, world_normal, world_position, world_tangent};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> materials: array<MaterialData>;
@group(0) @binding(2) var<storage, read> instances: array<Instance>;

struct Varying {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) current_clip: vec4<f32>,
    @location(5) previous_clip: vec4<f32>,
    @location(6) @interpolate(flat) material: u32,
}

@vertex
fn vs(
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv: vec2<f32>,
) -> Varying {
    let instance = instances[instance_index];
    let world = world_position(instance, position);
    var out: Varying;
    out.clip = frame.view_projection * vec4<f32>(world, 1.0);
    out.position = world;
    out.normal = world_normal(instance, normal);
    out.tangent = world_tangent(instance, tangent);
    out.uv = uv;
    out.current_clip = frame.unjittered_view_projection * vec4<f32>(world, 1.0);
    out.previous_clip = frame.previous_view_projection * vec4<f32>(previous_world_position(instance, position), 1.0);
    out.material = instance.material.x;
    return out;
}

struct Prepass {
    normal: vec2<f32>,
    motion: vec2<f32>,
    material: u32,
    roughness: f32,
    albedo: vec4<f32>,
}

fn prepass(in: Varying, front_facing: bool) -> Prepass {
    var input: SurfaceInput;
    input.position = in.position;
    input.normal = in.normal;
    input.tangent = in.tangent;
    input.uv = in.uv;
    input.uv_dx = dpdx(in.uv);
    input.uv_dy = dpdy(in.uv);
    input.color = vec4<f32>(1.0);
    input.front_facing = front_facing;
    let surface = load_surface(materials[in.material], input);
    if (!passes_alpha_test(surface)) {
        discard;
    }
    var out: Prepass;
    out.normal = oct_encode(surface.normal);
    out.motion = motion_vector(in.current_clip, in.previous_clip, frame.render_size);
    out.material = in.material;
    out.roughness = surface.roughness;
    out.albedo = vec4<f32>(surface.base_color, surface.metallic);
    return out;
}

struct BaseOutput {
    @location(0) normal: vec2<f32>,
    @location(1) motion: vec2<f32>,
    @location(2) material: u32,
}

struct FullOutput {
    @location(0) normal: vec2<f32>,
    @location(1) motion: vec2<f32>,
    @location(2) material: u32,
    @location(3) roughness: f32,
    @location(4) albedo: vec4<f32>,
}

@fragment
fn fs(in: Varying, @builtin(front_facing) front_facing: bool) -> BaseOutput {
    let result = prepass(in, front_facing);
    return BaseOutput(result.normal, result.motion, result.material);
}

@fragment
fn fs_full(in: Varying, @builtin(front_facing) front_facing: bool) -> FullOutput {
    let result = prepass(in, front_facing);
    return FullOutput(result.normal, result.motion, result.material, result.roughness, result.albedo);
}
