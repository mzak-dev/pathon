// Depth-only pass of one cascade: position stream in, light clip space out. The model matrix of caster `i` is
// `models[i]`; direct draws put the caster in `first_instance`, indirect draws in the argument's `first_instance`.

struct Cascade {
    view_projection: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> cascade: Cascade;
@group(0) @binding(1) var<storage, read> models: array<mat4x4<f32>>;

// One triangle that covers the whole viewport at the far plane (0 in reverse-Z): drawn with a depth test that always
// passes, it clears the tile of the atlas the viewport is set to, so a tile can be redrawn without touching the others.
@vertex
fn vs_clear(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - vec2<f32>(1.0), 0.0, 1.0);
}

@vertex
fn vs_depth(@location(0) position: vec3<f32>, @builtin(instance_index) caster: u32) -> @builtin(position) vec4<f32> {
    return cascade.view_projection * models[caster] * vec4<f32>(position, 1.0);
}
