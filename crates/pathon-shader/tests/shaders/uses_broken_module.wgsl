import package::broken::shade;

@fragment
fn fs() -> @location(0) vec4<f32> {
    let level = shade(0.5);
    return vec4<f32>(level, level, level, 1.0);
}
