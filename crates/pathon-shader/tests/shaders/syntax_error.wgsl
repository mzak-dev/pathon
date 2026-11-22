// A missing closing parenthesis on line 5.
@vertex
fn vs(@location(0) position: vec2<f32>) -> @builtin(position) vec4<f32> {
    let scaled = position * 2.0;
    return vec4<f32>(scaled, 0.0, 1.0;
}
