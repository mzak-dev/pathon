// The module is registered as `colors`; line 2 spells it `colours`.
import package::colours::tint;

@fragment
fn fs() -> @location(0) vec4<f32> {
    return tint();
}
