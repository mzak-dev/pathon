@group(0) @binding(4) var scene_depth: texture_depth_2d;

// Reverse-Z: geometry has a depth above the cleared far plane.
fn is_sky_pixel(pixel: vec2<i32>) -> bool {
    return textureLoad(scene_depth, pixel, 0) <= 0.0;
}
