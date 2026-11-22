struct FullscreenOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> FullscreenOutput {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var output: FullscreenOutput;
    output.position = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    output.uv = vec2<f32>(corner.x, 1.0 - corner.y);
    return output;
}
