// Lights the faces of the solid cells: the direct light of the scene's lights (with shadows from the voxels
// themselves), the light the probes found there one frame ago (the second and later bounces) and the emission.
// A slice of the cells is lit per frame (`GiPass::relight`), so the cost is a fraction of the cells every frame.
import package::gi::common::{GiVolume, PI, pack_rgb9e5, unpack_rgb9e5};
import package::gi::sample::gi_sample;

// GI_BINDINGS
// GI_VOXELS

const KIND_DIRECTIONAL: f32 = 0.0;
const KIND_SPOT: f32 = 2.0;
// A small emitter of an area light: it shines with the cosine of the angle to its axis, on one side or on both.
const KIND_COSINE: f32 = 3.0;
const KIND_COSINE_TWO_SIDED: f32 = 4.0;

struct LightSample {
    direction: vec3<f32>,
    radiance: vec3<f32>,
    distance: f32,
}

fn range_window(distance_square: f32, inverse_range_square: f32) -> f32 {
    let factor = distance_square * inverse_range_square;
    let smooth_factor = saturate(1.0 - factor * factor);
    return smooth_factor * smooth_factor;
}

// The light as `pbr::lights::light_sample` sees it, plus the distance to it (a long one for a directional light).
fn sample_light(light: GpuLight, position: vec3<f32>) -> LightSample {
    var result: LightSample;
    if (light.direction_kind.w == KIND_DIRECTIONAL) {
        result.direction = light.direction_kind.xyz;
        result.radiance = light.color_intensity.rgb;
        result.distance = gi_pass.reach.x;
        return result;
    }
    let to_light = light.position_range.xyz - position;
    let distance_square = dot(to_light, to_light);
    result.direction = to_light * inverseSqrt(max(distance_square, 1.0e-8));
    result.distance = sqrt(distance_square);
    var attenuation = range_window(distance_square, light.spot.z) / max(distance_square, 1.0e-4);
    if (light.direction_kind.w == KIND_SPOT) {
        let cone = saturate(dot(-light.direction_kind.xyz, result.direction) * light.spot.x + light.spot.y);
        attenuation = attenuation * cone * cone;
    }
    if (light.direction_kind.w >= KIND_COSINE) {
        let cosine = dot(light.direction_kind.xyz, -result.direction);
        attenuation = attenuation * select(max(cosine, 0.0), abs(cosine), light.direction_kind.w == KIND_COSINE_TWO_SIDED);
    }
    result.radiance = light.color_intensity.rgb * attenuation;
    return result;
}

// One thread per face of a solid cell of this frame's slice: the cells of the list whose number leaves `slice` modulo
// `slices`. The dispatch is two-dimensional, because a grid of millions of cells has more threads than a row holds.
@compute @workgroup_size(64)
fn light_voxels(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let thread = id.x + id.y * groups.x * 64u;
    let face = thread % 6u;
    let list_index = (thread / 6u) * gi_pass.relight.y + gi_pass.relight.x;
    if (list_index >= atomicLoad(&solid_cells[0])) {
        return;
    }
    let index = atomicLoad(&solid_cells[1u + list_index]);
    let size = gi_grid.origin_size.w;
    let cell = vec3<i32>(
        i32(index % gi_grid.dims.x),
        i32((index / gi_grid.dims.x) % gi_grid.dims.y),
        i32(index / (gi_grid.dims.x * gi_grid.dims.y)),
    );
    let albedo = albedo_of(voxel_albedo[index]);
    let emissive = unpack_rgb9e5(voxel_emissive[index]);
    let center = cell_center(cell);
    {
        let normal = face_normal(face);
        let neighbour = cell + vec3<i32>(normal);
        if (cell_solid(neighbour)) {
            voxel_radiance[index * 6u + face] = 0u;
            return;
        }
        // Just outside the face, away from the cell the ray would otherwise start in.
        let origin = center + normal * (0.6 * size);
        var irradiance = vec3<f32>(0.0);
        for (var i = 0u; i < gi_pass.relight.z; i++) {
            let light = sample_light(gi_lights[i], origin);
            let cosine = dot(normal, light.direction);
            if (cosine <= 0.0 || all(light.radiance <= vec3<f32>(0.0))) {
                continue;
            }
            if (!trace_voxels(origin, light.direction, light.distance).hit) {
                irradiance += light.radiance * cosine;
            }
        }
        let bounced = gi_sample(
            gi_volume,
            previous_irradiance,
            previous_visibility,
            gi_sampler,
            origin,
            normal,
            normal,
            normal,
        );
        let indirect = bounced.irradiance * bounced.confidence * gi_pass.blend.z;
        voxel_radiance[index * 6u + face] = pack_rgb9e5(emissive + albedo * (irradiance / PI + indirect));
    }
}
