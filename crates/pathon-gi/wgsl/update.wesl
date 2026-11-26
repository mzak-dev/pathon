// Folds the rays of the probes updated this frame into the irradiance and the distance moments of their tiles, with
// the previous frame's value as the hysteresis (Majercik et al. 2019). One thread per atlas texel; the one-texel
// border of a tile repeats the wrapped interior texel, so a bilinear fetch near the edge of the octahedral map is
// continuous. The probes outside this frame's batch keep their texels.
import package::gi::common::{GiVolume, IRRADIANCE_RES, IRRADIANCE_TILE, VISIBILITY_RES, VISIBILITY_TILE, oct_decode};

// GI_BINDINGS

const RAY_SKY: f32 = 1.0;
const RAY_INSIDE: f32 = 2.0;

// The interior texel (0..res-1) a texel of a tile with a border (0..res+1) stands for: itself inside, the wrapped
// opposite texel on the border (the octahedral map folds over its edges).
fn interior_texel(local: vec2<u32>, res: u32) -> vec2<u32> {
    var texel = vec2<i32>(local) - vec2<i32>(1);
    let last = i32(res) - 1;
    let on_x = texel.x < 0 || texel.x > last;
    let on_y = texel.y < 0 || texel.y > last;
    if (on_x && on_y) {
        texel = vec2<i32>(last) - clamp(texel, vec2<i32>(0), vec2<i32>(last));
        return vec2<u32>(texel);
    }
    if (on_x) {
        texel = vec2<i32>(clamp(texel.x, 0, last), last - texel.y);
    } else if (on_y) {
        texel = vec2<i32>(last - texel.x, clamp(texel.y, 0, last));
    }
    return vec2<u32>(texel);
}

struct Folded {
    color: vec3<f32>,
    sky: f32,
    weight: f32,
    inside: bool,
}

fn fold_irradiance(slot: u32, direction: vec3<f32>) -> Folded {
    var folded: Folded;
    folded.color = vec3<f32>(0.0);
    folded.sky = 0.0;
    folded.weight = 0.0;
    folded.inside = false;
    let rays = gi_pass.frame.w;
    for (var ray = 0u; ray < rays; ray++) {
        let first = (slot * rays + ray) * 2u;
        let radiance = gi_rays[first];
        let info = gi_rays[first + 1u];
        if (info.w == RAY_INSIDE) {
            folded.inside = true;
            return folded;
        }
        let weight = max(0.0, dot(direction, info.xyz));
        folded.color += weight * radiance.rgb;
        folded.sky += select(0.0, weight, info.w == RAY_SKY);
        folded.weight += weight;
    }
    return folded;
}

// Position of `probe` in this frame's batch (the batch is a run of probes that wraps around the end).
fn batch_slot(probe: u32, probes: u32) -> u32 {
    return (probe + probes - gi_pass.frame.y % probes) % probes;
}

@compute @workgroup_size(8, 8)
fn update_irradiance(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(gi_volume.atlas.xy);
    if (any(id.xy >= size)) {
        return;
    }
    let tile = id.xy / IRRADIANCE_TILE;
    let probes = gi_volume.counts.x * gi_volume.counts.y * gi_volume.counts.z;
    let probe = tile.y * gi_volume.counts.w + tile.x;
    let previous = textureLoad(previous_irradiance, vec2<i32>(id.xy), 0);
    if (probe >= probes) {
        textureStore(next_irradiance, vec2<i32>(id.xy), vec4<f32>(0.0));
        return;
    }
    let slot = batch_slot(probe, probes);
    if (slot >= gi_pass.frame.z) {
        textureStore(next_irradiance, vec2<i32>(id.xy), previous);
        return;
    }
    let interior = interior_texel(id.xy % IRRADIANCE_TILE, IRRADIANCE_RES);
    let direction = oct_decode((vec2<f32>(interior) + vec2<f32>(0.5)) / f32(IRRADIANCE_RES) * 2.0 - vec2<f32>(1.0));
    let folded = fold_irradiance(slot, direction);
    if (folded.inside) {
        textureStore(next_irradiance, vec2<i32>(id.xy), vec4<f32>(0.0));
        return;
    }
    if (folded.weight <= 1.0e-5) {
        textureStore(next_irradiance, vec2<i32>(id.xy), previous);
        return;
    }
    let value = vec4<f32>(folded.color, folded.sky) / folded.weight;
    textureStore(next_irradiance, vec2<i32>(id.xy), mix(value, previous, gi_pass.blend.x));
}

@compute @workgroup_size(8, 8)
fn update_visibility(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(gi_volume.atlas.zw);
    if (any(id.xy >= size)) {
        return;
    }
    let tile = id.xy / VISIBILITY_TILE;
    let probes = gi_volume.counts.x * gi_volume.counts.y * gi_volume.counts.z;
    let probe = tile.y * gi_volume.counts.w + tile.x;
    let previous = textureLoad(previous_visibility, vec2<i32>(id.xy), 0);
    if (probe >= probes) {
        textureStore(next_visibility, vec2<i32>(id.xy), vec4<f32>(0.0));
        return;
    }
    let slot = batch_slot(probe, probes);
    if (slot >= gi_pass.frame.z) {
        textureStore(next_visibility, vec2<i32>(id.xy), previous);
        return;
    }
    let interior = interior_texel(id.xy % VISIBILITY_TILE, VISIBILITY_RES);
    let direction = oct_decode((vec2<f32>(interior) + vec2<f32>(0.5)) / f32(VISIBILITY_RES) * 2.0 - vec2<f32>(1.0));
    let rays = gi_pass.frame.w;
    let reach = gi_volume.bias.z;
    var moments = vec2<f32>(0.0);
    var total = 0.0;
    for (var ray = 0u; ray < rays; ray++) {
        let first = (slot * rays + ray) * 2u;
        let info = gi_rays[first + 1u];
        if (info.w == RAY_INSIDE) {
            textureStore(next_visibility, vec2<i32>(id.xy), vec4<f32>(0.0));
            return;
        }
        let weight = pow(max(0.0, dot(direction, info.xyz)), gi_pass.blend.w);
        let travelled = min(gi_rays[first].w, reach) / reach;
        moments += weight * vec2<f32>(travelled, travelled * travelled);
        total += weight;
    }
    if (total <= 1.0e-5) {
        textureStore(next_visibility, vec2<i32>(id.xy), previous);
        return;
    }
    let value = vec4<f32>(moments / total, 0.0, 0.0);
    textureStore(next_visibility, vec2<i32>(id.xy), mix(value, previous, gi_pass.blend.x));
}
