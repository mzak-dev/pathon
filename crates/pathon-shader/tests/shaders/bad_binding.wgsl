// Two resources share group 0, binding 0; the second one is on line 4.
struct Tint { color: vec4<f32> }
@group(0) @binding(0) var<uniform> first: Tint;
@group(0) @binding(0) var<uniform> second: Tint;

@fragment
fn fs() -> @location(0) vec4<f32> {
    return first.color + second.color;
}
