@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var exposure_texture: texture_2d<f32>;

fn fetch(uv: vec2<f32>) -> vec4<f32> {
    let level = i32(min(params.level, textureNumLevels(source) - 1u));
    let size = vec2<i32>(textureDimensions(source, level));
    let coord = clamp(vec2<i32>(uv * vec2<f32>(size)), vec2<i32>(0, 0), size - vec2<i32>(1, 1));
    return textureLoad(source, coord, level);
}

fn fetch_id(uv: vec2<f32>) -> u32 {
    return 0u;
}

fn exposure_multiplier() -> f32 {
    if (params.use_exposure_texture != 0u) {
        return textureLoad(exposure_texture, vec2<i32>(0, 0), 0).r;
    }
    return params.exposure;
}
