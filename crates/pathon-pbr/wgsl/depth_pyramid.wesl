// Hi-Z pyramid: mip 0 is the reverse-Z depth, every next mip keeps the minimum (the farthest surface) of the texels
// it covers. A mip with an odd source size also takes the extra last row and column, so no depth is dropped.
struct Params {
    source_size: vec2<u32>,
    target_size: vec2<u32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var previous: texture_2d<f32>;
@group(0) @binding(3) var output_mip: texture_storage_2d<r32float, write>;

@compute @workgroup_size(8, 8)
fn copy_depth(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.target_size.x || id.y >= params.target_size.y) {
        return;
    }
    textureStore(output_mip, vec2<i32>(id.xy), vec4<f32>(textureLoad(depth, vec2<i32>(id.xy), 0), 0.0, 0.0, 0.0));
}

@compute @workgroup_size(8, 8)
fn downsample(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.target_size.x || id.y >= params.target_size.y) {
        return;
    }
    let last = params.source_size - vec2<u32>(1u);
    let first = id.xy * 2u;
    var span = vec2<u32>(2u);
    if (id.x + 1u == params.target_size.x && (params.source_size.x & 1u) == 1u) {
        span.x = 3u;
    }
    if (id.y + 1u == params.target_size.y && (params.source_size.y & 1u) == 1u) {
        span.y = 3u;
    }
    var farthest = 1.0;
    for (var dy = 0u; dy < span.y; dy++) {
        for (var dx = 0u; dx < span.x; dx++) {
            let texel = min(first + vec2<u32>(dx, dy), last);
            farthest = min(farthest, textureLoad(previous, vec2<i32>(texel), 0).r);
        }
    }
    textureStore(output_mip, vec2<i32>(id.xy), vec4<f32>(farthest, 0.0, 0.0, 0.0));
}
