// Visibility of light `light_index` at `position`: 1 lit, 0 in shadow. `pathon-shadow` (M4-07) replaces this module;
// without it every light is unshadowed.
fn shadow_factor(light_index: u32, position: vec3<f32>) -> f32 {
    return 1.0;
}
