@group(0) @binding(0) var source: texture_2d_array<f32>;
@group(0) @binding(1) var destination: texture_storage_2d_array<rgba16float, write>;

// Box filter: every destination texel is exactly four source texels of one face, so no face edge is crossed.
@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(destination);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let origin = vec2<i32>(id.xy) * 2;
    let layer = i32(id.z);
    var sum = textureLoad(source, origin, layer, 0);
    sum += textureLoad(source, origin + vec2<i32>(1, 0), layer, 0);
    sum += textureLoad(source, origin + vec2<i32>(0, 1), layer, 0);
    sum += textureLoad(source, origin + vec2<i32>(1, 1), layer, 0);
    textureStore(destination, vec2<i32>(id.xy), layer, sum * 0.25);
}
