@group(0) @binding(1) var scene_depth: texture_depth_2d;

// Reverse-Z: a larger depth is nearer. The tolerance is relative, so it holds at any distance.
fn occluded(fragment: vec4<f32>) -> bool {
    let size = vec2<i32>(textureDimensions(scene_depth));
    let coord = clamp(vec2<i32>(fragment.xy * frame.depth_scale), vec2<i32>(0, 0), size - vec2<i32>(1, 1));
    return fragment.z * (1.0 + frame.depth_bias) < textureLoad(scene_depth, coord, 0);
}
