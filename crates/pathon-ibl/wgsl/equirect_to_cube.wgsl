struct Params {
    face_size: u32,
    samples_per_axis: u32,
    padding: vec2<u32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var equirect: texture_2d<f32>;
@group(0) @binding(2) var cube: texture_storage_2d_array<rgba16float, write>;

// Longitude 0.5 is the -Z direction, latitude 0 the +Y pole: the picture centre looks down -Z.
fn equirect_uv(direction: vec3<f32>) -> vec2<f32> {
    let longitude = atan2(direction.x, -direction.z) / (2.0 * PI) + 0.5;
    let latitude = acos(clamp(direction.y, -1.0, 1.0)) / PI;
    return vec2<f32>(longitude, latitude);
}

fn equirect_texel(x: i32, y: i32) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(equirect));
    let wrapped_x = ((x % size.x) + size.x) % size.x;
    return textureLoad(equirect, vec2<i32>(wrapped_x, clamp(y, 0, size.y - 1)), 0).rgb;
}

// Bilinear filter by hand: the picture wraps around in longitude and stops at the poles, which no sampler address
// mode expresses.
fn sample_equirect(direction: vec3<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(equirect));
    let position = equirect_uv(direction) * size - vec2<f32>(0.5);
    let base = floor(position);
    let fraction = position - base;
    let x = i32(base.x);
    let y = i32(base.y);
    let top = mix(equirect_texel(x, y), equirect_texel(x + 1, y), fraction.x);
    let bottom = mix(equirect_texel(x, y + 1), equirect_texel(x + 1, y + 1), fraction.x);
    return mix(top, bottom, fraction.y);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.face_size || id.y >= params.face_size) {
        return;
    }
    // A cube texel can cover many equirect texels: average a grid of sub-samples so a small bright sun survives.
    let count = params.samples_per_axis;
    var sum = vec3<f32>(0.0);
    for (var j = 0u; j < count; j++) {
        for (var i = 0u; i < count; i++) {
            let x = f32(id.x) + (f32(i) + 0.5) / f32(count);
            let y = f32(id.y) + (f32(j) + 0.5) / f32(count);
            sum += sample_equirect(texel_direction(id.z, x, y, f32(params.face_size)));
        }
    }
    let average = min(sum / f32(count * count), vec3<f32>(MAX_RADIANCE));
    textureStore(cube, vec2<i32>(id.xy), i32(id.z), vec4<f32>(average, 1.0));
}
