// Directional light shadows from cascaded maps. The module declares no bindings: the shader that owns the entry point
// declares them (pathon_shadow::ShadowBindings) and passes them in. Depth maps are reverse-Z (1 at the light), so a
// receiver is lit when its depth is greater than or equal to the stored one.

const SHADOW_MAX_CASCADES: u32 = 4u;
const SHADOW_FILTER_HARD: u32 = 0u;
const SHADOW_FILTER_PCF3: u32 = 1u;
const SHADOW_FILTER_PCF5: u32 = 2u;
const SHADOW_PCF5_RADIUS_TEXELS: f32 = 2.5;
const SHADOW_TAU: f32 = 6.28318530717958647692;

struct ShadowCascade {
    // World to clip space of the light: x and y in -1..1 over the map, z the reverse-Z depth in 0..1.
    view_projection: mat4x4<f32>,
    // x: metres per texel, y: normal offset in metres.
    params: vec4<f32>,
}

struct ShadowCascades {
    cascades: array<ShadowCascade, 4>,
    // xyz: the direction the light shines along, normalised.
    light_direction: vec4<f32>,
    // x: light index, y: cascade count, z: filter, w: flags (bit 0: rotate the filter per pixel).
    counts: vec4<u32>,
    // Far end of every cascade as a view distance in metres.
    splits: vec4<f32>,
    // x: blend fraction, y: fade fraction, z: map resolution, w: 1 / resolution.
    tuning: vec4<f32>,
}

var<private> shadow_poisson: array<vec2<f32>, 16> = array<vec2<f32>, 16>(
    vec2<f32>(-0.94201624, -0.39906216),
    vec2<f32>(0.94558609, -0.76890725),
    vec2<f32>(-0.094184101, -0.92938870),
    vec2<f32>(0.34495938, 0.29387760),
    vec2<f32>(-0.91588581, 0.45771432),
    vec2<f32>(-0.81544232, -0.87912464),
    vec2<f32>(-0.38277543, 0.27676845),
    vec2<f32>(0.97484398, 0.75648379),
    vec2<f32>(0.44323325, -0.97511554),
    vec2<f32>(0.53742981, -0.47373420),
    vec2<f32>(-0.26496911, -0.41893023),
    vec2<f32>(0.79197514, 0.19090188),
    vec2<f32>(-0.24188840, 0.99706507),
    vec2<f32>(-0.81409955, 0.91437590),
    vec2<f32>(0.19984126, 0.78641367),
    vec2<f32>(0.14383161, -0.14100790),
);

fn shadow_interleaved_gradient_noise(pixel: vec2<f32>) -> f32 {
    let m = vec3<f32>(0.06711056, 0.00583715, 52.9829189);
    return fract(m.z * fract(dot(pixel, m.xy)));
}

// Texture coordinates (xy) and the depth to compare against (z) of a world position in a cascade. The position is
// first moved along its normal by sin(theta) * the cascade's normal bias, theta being the angle to the light
// (normal-offset shadows, Holbert 2011): the bias grows where a surface is seen at a grazing angle.
fn shadow_light_space(
    shadows: ShadowCascades,
    cascade: u32,
    world_position: vec3<f32>,
    normal: vec3<f32>,
) -> vec3<f32> {
    let cos_theta = clamp(dot(normal, -shadows.light_direction.xyz), 0.0, 1.0);
    let sin_theta = sqrt(1.0 - cos_theta * cos_theta);
    let moved = world_position + normal * (sin_theta * shadows.cascades[cascade].params.y);
    let clip = shadows.cascades[cascade].view_projection * vec4<f32>(moved, 1.0);
    return vec3<f32>(clip.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5), clip.z);
}

fn shadow_tap(
    shadow_map: texture_depth_2d_array,
    shadow_sampler: sampler_comparison,
    layer: u32,
    uv: vec2<f32>,
    depth: f32,
) -> f32 {
    return textureSampleCompareLevel(shadow_map, shadow_sampler, uv, layer, depth);
}

// A box filter three texels wide, in four hardware taps. Along one axis the box around the sample point p = i + s
// (i whole texels, s the fraction) covers the texels i-1, i, i+1 and i+2 by a, 1, 1 and c, with a = min(1, 1.5 - s) and
// c = max(0, s - 0.5), which add up to 3. A bilinear tap between two texels returns their weighted average, so one tap
// at the weighted centre of (i-1, i) and one of (i+1, i+2) stand in for four texel reads; the 2D filter is the product of
// the two axes.
fn shadow_pcf3(
    shadow_map: texture_depth_2d_array,
    shadow_sampler: sampler_comparison,
    layer: u32,
    position: vec3<f32>,
    resolution: f32,
) -> f32 {
    let depth = saturate(position.z);
    let p = clamp(position.xy, vec2<f32>(-1.0), vec2<f32>(2.0)) * resolution;
    let whole = floor(p);
    let fraction = p - whole;

    let low = min(vec2<f32>(1.0), vec2<f32>(1.5) - fraction);
    let high = max(vec2<f32>(0.0), fraction - vec2<f32>(0.5));
    let first_weight = low + vec2<f32>(1.0);
    let second_weight = vec2<f32>(1.0) + high;
    let first = (whole - vec2<f32>(0.5) + vec2<f32>(1.0) / first_weight) / resolution;
    let second = (whole + vec2<f32>(1.5) + high / second_weight) / resolution;

    var sum = first_weight.x * first_weight.y * shadow_tap(shadow_map, shadow_sampler, layer, first, depth);
    sum += second_weight.x * first_weight.y *
        shadow_tap(shadow_map, shadow_sampler, layer, vec2<f32>(second.x, first.y), depth);
    sum += first_weight.x * second_weight.y *
        shadow_tap(shadow_map, shadow_sampler, layer, vec2<f32>(first.x, second.y), depth);
    sum += second_weight.x * second_weight.y * shadow_tap(shadow_map, shadow_sampler, layer, second, depth);
    return sum / 9.0;
}

// Sixteen taps of a Poisson disc, turned by `rotation` (cosine, sine).
fn shadow_pcf5(
    shadow_map: texture_depth_2d_array,
    shadow_sampler: sampler_comparison,
    layer: u32,
    position: vec3<f32>,
    resolution: f32,
    rotation: vec2<f32>,
) -> f32 {
    let depth = saturate(position.z);
    let radius = SHADOW_PCF5_RADIUS_TEXELS / resolution;
    var sum = 0.0;
    for (var tap = 0u; tap < 16u; tap += 1u) {
        let disc = shadow_poisson[tap];
        let turned = vec2<f32>(disc.x * rotation.x - disc.y * rotation.y, disc.x * rotation.y + disc.y * rotation.x);
        sum += shadow_tap(shadow_map, shadow_sampler, layer, position.xy + turned * radius, depth);
    }
    return sum * 0.0625;
}

// Visibility of a world position in one cascade, 0 in shadow and 1 lit; positions outside the map are lit.
fn shadow_sample_cascade(
    shadows: ShadowCascades,
    shadow_map: texture_depth_2d_array,
    shadow_sampler: sampler_comparison,
    cascade: u32,
    world_position: vec3<f32>,
    normal: vec3<f32>,
    pixel: vec2<f32>,
) -> f32 {
    let position = shadow_light_space(shadows, cascade, world_position, normal);
    if (any(position.xy < vec2<f32>(0.0)) || any(position.xy > vec2<f32>(1.0))) {
        return 1.0;
    }
    let resolution = shadows.tuning.z;
    let filter_mode = shadows.counts.z;
    if (filter_mode == SHADOW_FILTER_HARD) {
        return shadow_tap(shadow_map, shadow_sampler, cascade, position.xy, saturate(position.z));
    }
    if (filter_mode == SHADOW_FILTER_PCF3) {
        return shadow_pcf3(shadow_map, shadow_sampler, cascade, position, resolution);
    }
    var rotation = vec2<f32>(1.0, 0.0);
    if ((shadows.counts.w & 1u) != 0u) {
        let angle = SHADOW_TAU * shadow_interleaved_gradient_noise(pixel);
        rotation = vec2<f32>(cos(angle), sin(angle));
    }
    return shadow_pcf5(shadow_map, shadow_sampler, cascade, position, resolution, rotation);
}

// The cascade of a view distance: the number of splits it lies beyond.
fn shadow_cascade_index(shadows: ShadowCascades, view_depth: f32) -> u32 {
    var index = 0u;
    for (var split = 0u; split + 1u < shadows.counts.y; split += 1u) {
        if (view_depth > shadows.splits[split]) {
            index = split + 1u;
        }
    }
    return index;
}

// The visibility of the light `light_index` at a surface: 0 fully in shadow, 1 lit. `world_position` and `normal`
// (the geometric normal, unit length) are in world space, `view_depth` is the distance along the camera's forward axis
// in metres and `pixel` is the fragment's `@builtin(position).xy`, which only decorrelates the rotated filter. A light
// that is not the shadowed one, or a position beyond the shadowed range, is lit.
fn shadow_factor_directional(
    shadows: ShadowCascades,
    shadow_map: texture_depth_2d_array,
    shadow_sampler: sampler_comparison,
    light_index: u32,
    world_position: vec3<f32>,
    normal: vec3<f32>,
    view_depth: f32,
    pixel: vec2<f32>,
) -> f32 {
    let count = shadows.counts.y;
    if (light_index != shadows.counts.x || count == 0u) {
        return 1.0;
    }
    let far = shadows.splits[count - 1u];
    if (view_depth >= far) {
        return 1.0;
    }

    let cascade = shadow_cascade_index(shadows, view_depth);
    var visibility = shadow_sample_cascade(shadows, shadow_map, shadow_sampler, cascade, world_position, normal, pixel);

    // Over the last share of a cascade the next one takes over, so the seam between two resolutions is not a line.
    if (cascade + 1u < count) {
        let end = shadows.splits[cascade];
        let begin = select(0.0, shadows.splits[max(cascade, 1u) - 1u], cascade > 0u);
        let width = max(shadows.tuning.x * (end - begin), 1.0e-4);
        let blend = saturate((view_depth - (end - width)) / width);
        if (blend > 0.0) {
            let next = shadow_sample_cascade(
                shadows,
                shadow_map,
                shadow_sampler,
                cascade + 1u,
                world_position,
                normal,
                pixel,
            );
            visibility = mix(visibility, next, blend);
        }
    }

    let fade_width = max(shadows.tuning.y * far, 1.0e-4);
    return mix(1.0, visibility, saturate((far - view_depth) / fade_width));
}
