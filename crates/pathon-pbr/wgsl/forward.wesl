// Forward colour pass: the standard material under the directional lights, the clustered point and spot lights and the
// image-based environment. Writes `hdr` (linear, no exposure) and, in `fs_split`, the `diffuse` and `specular` parts.
import package::ibl::shading::{IblInput, ibl_evaluate};
import package::material::standard::{MaterialData, evaluate, load_surface, output_alpha, passes_alpha_test};
import package::material::surface::{LightSample, Surface, SurfaceInput};
import package::pbr::common::{Frame, Instance, cluster_index, world_normal, world_position, world_tangent};
import package::pbr::hooks::shadow_factor;
import package::pbr::lights::{GpuLight, light_sample};
import package::pbr::occlusion::{gtao_multi_bounce, specular_ao_lagarde};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> materials: array<MaterialData>;
@group(0) @binding(2) var<storage, read> instances: array<Instance>;
@group(0) @binding(3) var<storage, read> lights: array<GpuLight>;
@group(0) @binding(4) var<storage, read> cluster_ranges: array<vec2<u32>>;
@group(0) @binding(5) var<storage, read> cluster_indices: array<u32>;
// IBL_BINDINGS

struct Varying {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) @interpolate(flat) material: u32,
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
    out.material = instance.material.x;
    return out;
}

fn add_light(
    surface: Surface,
    view: vec3<f32>,
    index: u32,
    diffuse: ptr<function, vec3<f32>>,
    specular: ptr<function, vec3<f32>>,
) {
    let light = lights[index];
    let lit = light_sample(light, surface.position);
    let visibility = shadow_factor(index, surface.position);
    if (visibility <= 0.0) {
        return;
    }
    let lobes = evaluate(surface, view, LightSample(lit.direction, lit.radiance * visibility));
    *diffuse += lobes.diffuse;
    *specular += lobes.specular;
}

struct Shaded {
    diffuse: vec3<f32>,
    specular: vec3<f32>,
    emissive: vec3<f32>,
    alpha: f32,
}

fn shade(in: Varying, front_facing: bool) -> Shaded {
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
    let view = normalize(frame.camera_position.xyz - in.position);

    var diffuse = vec3<f32>(0.0);
    var specular = vec3<f32>(0.0);
    let directional = frame.light_counts.x;
    for (var light = 0u; light < directional; light++) {
        add_light(surface, view, light, &diffuse, &specular);
    }
    if (frame.light_counts.w != 0u) {
        for (var light = directional; light < frame.light_counts.y; light++) {
            add_light(surface, view, light, &diffuse, &specular);
        }
    } else {
        let view_depth = -(frame.world_to_view * vec4<f32>(in.position, 1.0)).z;
        let range = cluster_ranges[cluster_index(frame, in.clip.xy, view_depth)];
        for (var entry = 0u; entry < range.y; entry++) {
            add_light(surface, view, cluster_indices[range.x + entry], &diffuse, &specular);
        }
    }

    let n_dot_v = max(dot(surface.normal, view), 0.0001);
    let environment = ibl_evaluate(
        ibl_irradiance,
        ibl_prefiltered,
        ibl_brdf_lut,
        ibl_sampler,
        IblInput(surface.normal, view, surface.diffuse_color, surface.f0, surface.f90, surface.roughness),
    );
    // Ambient occlusion darkens the diffuse environment, and the specular one through Lagarde's term (2014), both with
    // the multi-bounce correction of Jimenez et al. (2016) so bright surfaces do not go black in creases.
    let specular_occlusion = specular_ao_lagarde(n_dot_v, surface.occlusion, surface.roughness);
    diffuse += environment.diffuse * gtao_multi_bounce(surface.occlusion, surface.diffuse_color)
        * frame.environment_intensity;
    specular += environment.specular * gtao_multi_bounce(specular_occlusion, surface.f0)
        * frame.environment_intensity;

    var out: Shaded;
    out.diffuse = diffuse;
    out.specular = specular;
    out.emissive = surface.emissive;
    out.alpha = output_alpha(surface);
    return out;
}

@fragment
fn fs(in: Varying, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    let shaded = shade(in, front_facing);
    let radiance = shaded.emissive + shaded.diffuse + shaded.specular;
    var color = vec4<f32>(radiance, 1.0);
    // Blended surfaces are written premultiplied, so the blend state needs no separate colour factor.
    @if(transparent) {
        color = vec4<f32>(radiance * shaded.alpha, shaded.alpha);
    }
    return color;
}

struct SplitOutput {
    @location(0) hdr: vec4<f32>,
    @location(1) diffuse: vec4<f32>,
    @location(2) specular: vec4<f32>,
}

@fragment
fn fs_split(in: Varying, @builtin(front_facing) front_facing: bool) -> SplitOutput {
    let shaded = shade(in, front_facing);
    // Emission rides with `specular` in the split slots: it must not be blurred as subsurface diffuse.
    let specular = shaded.specular + shaded.emissive;
    return SplitOutput(
        vec4<f32>(shaded.emissive + shaded.diffuse + shaded.specular, shaded.alpha),
        vec4<f32>(shaded.diffuse, 1.0),
        vec4<f32>(specular, 1.0),
    );
}
