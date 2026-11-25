@group(0) @binding(1) var source: texture_2d<u32>;

fn fetch_id(uv: vec2<f32>) -> u32 {
    let size = vec2<i32>(textureDimensions(source));
    let coord = clamp(vec2<i32>(uv * vec2<f32>(size)), vec2<i32>(0, 0), size - vec2<i32>(1, 1));
    return textureLoad(source, coord, 0).r;
}

fn fetch(uv: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(f32(fetch_id(uv)), 0.0, 0.0, 1.0);
}

fn exposure_multiplier() -> f32 {
    return params.exposure;
}
