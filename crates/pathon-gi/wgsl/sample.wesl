// Diffuse indirect light from the probe volume. The bindings are the consumer's (like `ibl::sample`): it declares
// the volume uniform, the two atlases and a linear clamp sampler (`GiBindings::wgsl_declarations`) and passes them in.
//
// Trilinear blend of the eight probes around the point, each weighted by a smooth backface term and a Chebyshev test
// on the distance moments (Majercik et al. 2019, "Dynamic Diffuse Global Illumination with Ray-Traced Irradiance
// Fields"; 2021, "Scaling Probe-Based Real-Time Dynamic Global Illumination for Production"): a probe on the other side
// of a wall has a mean distance shorter than the distance to the point, so its weight goes to zero and no light leaks.
//
// The irradiance is the cosine-weighted mean radiance (E / pi), the convention of `ibl::sample::ibl_diffuse`: multiply
// by the diffuse albedo to get the outgoing radiance.
import package::gi::common::{GiVolume, IRRADIANCE_RES, IRRADIANCE_TILE, VISIBILITY_RES, VISIBILITY_TILE, oct_encode, probe_index, probe_position, probe_tile};

struct GiSample {
    // Cosine-weighted mean incoming radiance around `normal`, scaled by the volume's indirect intensity.
    irradiance: vec3<f32>,
    // 0 where the volume knows nothing about the point (outside it, or every probe sees through a wall), 1 inside.
    confidence: f32,
    // Fraction of the hemisphere around `specular_direction` that sees the sky.
    sky_visibility: f32,
}

fn irradiance_texel(volume: GiVolume, tile: vec2<u32>, direction: vec3<f32>) -> vec2<f32> {
    let octahedral = oct_encode(direction) * 0.5 + vec2<f32>(0.5);
    return (vec2<f32>(tile * IRRADIANCE_TILE) + vec2<f32>(1.0) + octahedral * f32(IRRADIANCE_RES)) / volume.atlas.xy;
}

fn visibility_texel(volume: GiVolume, tile: vec2<u32>, direction: vec3<f32>) -> vec2<f32> {
    let octahedral = oct_encode(direction) * 0.5 + vec2<f32>(0.5);
    return (vec2<f32>(tile * VISIBILITY_TILE) + vec2<f32>(1.0) + octahedral * f32(VISIBILITY_RES)) / volume.atlas.zw;
}

// `position` and `normal` are the surface point and its normal, `view_direction` points from the point to the eye;
// `specular_direction` is where the sky visibility is read (the mirror direction, or the normal).
fn gi_sample(
    volume: GiVolume,
    irradiance: texture_2d<f32>,
    visibility: texture_2d<f32>,
    atlas_sampler: sampler,
    position: vec3<f32>,
    normal: vec3<f32>,
    view_direction: vec3<f32>,
    specular_direction: vec3<f32>,
) -> GiSample {
    var result: GiSample;
    result.irradiance = vec3<f32>(0.0);
    result.confidence = 0.0;
    result.sky_visibility = 1.0;

    let spacing = volume.origin_spacing.w;
    let biased = position + normal * volume.bias.x + view_direction * volume.bias.y;
    let last_cell = vec3<f32>(volume.counts.xyz - vec3<u32>(1u));
    let grid = (biased - volume.origin_spacing.xyz) / spacing;
    let outside = max(max(-grid, grid - last_cell), vec3<f32>(0.0));
    let edge = 1.0 - saturate(max(outside.x, max(outside.y, outside.z)) / max(volume.tuning.y, 1.0e-3));
    if (edge <= 0.0) {
        return result;
    }
    let inside = clamp(grid, vec3<f32>(0.0), last_cell);
    let base = min(floor(inside), max(last_cell - vec3<f32>(1.0), vec3<f32>(0.0)));
    let alpha = inside - base;

    var total = vec3<f32>(0.0);
    var sky = 0.0;
    var total_weight = 0.0;
    for (var corner = 0u; corner < 8u; corner++) {
        let offset = vec3<u32>(corner & 1u, (corner >> 1u) & 1u, (corner >> 2u) & 1u);
        let cell = min(vec3<u32>(base) + offset, volume.counts.xyz - vec3<u32>(1u));
        let probe = probe_position(volume, cell);
        let trilinear = (select(1.0 - alpha.x, alpha.x, offset.x == 1u))
            * (select(1.0 - alpha.y, alpha.y, offset.y == 1u))
            * (select(1.0 - alpha.z, alpha.z, offset.z == 1u));

        var weight = 1.0;
        let to_probe = probe - position;
        let direction_to_probe = to_probe / max(length(to_probe), 1.0e-5);
        let facing = max(1.0e-4, (dot(direction_to_probe, normal) + 1.0) * 0.5);
        weight = weight * (facing * facing + 0.2);

        let biased_to_probe = probe - biased;
        let distance = max(length(biased_to_probe), 1.0e-5);
        let tile = probe_tile(volume, probe_index(volume, cell));
        let moments = textureSampleLevel(
            visibility,
            atlas_sampler,
            visibility_texel(volume, tile, -biased_to_probe / distance),
            0.0,
        ).xy;
        let mean = moments.x * volume.bias.z;
        let mean_square = moments.y * volume.bias.z * volume.bias.z;
        if (distance > mean) {
            let variance = abs(mean_square - mean * mean);
            let gap = distance - mean;
            let chebyshev = variance / (variance + gap * gap);
            weight = weight * chebyshev * chebyshev * chebyshev;
        }
        weight = weight * max(trilinear, 1.0e-6);

        let probe_irradiance = textureSampleLevel(irradiance, atlas_sampler, irradiance_texel(volume, tile, normal), 0.0);
        let probe_sky = textureSampleLevel(irradiance, atlas_sampler, irradiance_texel(volume, tile, specular_direction), 0.0).a;
        total += weight * probe_irradiance.rgb;
        sky += weight * probe_sky;
        total_weight += weight;
    }
    if (total_weight > 0.0) {
        result.irradiance = total / total_weight * volume.bias.w;
        result.sky_visibility = sky / total_weight;
    }
    result.confidence = edge * smoothstep(1.0e-4, 2.0e-2, total_weight);
    return result;
}

fn gi_irradiance(
    volume: GiVolume,
    irradiance: texture_2d<f32>,
    visibility: texture_2d<f32>,
    atlas_sampler: sampler,
    position: vec3<f32>,
    normal: vec3<f32>,
    view_direction: vec3<f32>,
) -> vec3<f32> {
    let sampled = gi_sample(volume, irradiance, visibility, atlas_sampler, position, normal, view_direction, normal);
    return sampled.irradiance * sampled.confidence;
}
