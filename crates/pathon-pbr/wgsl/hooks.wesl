// Extra visibility of light `light_index` at `position` (1 lit, 0 in shadow) that the application adds; the cascaded
// shadow map of `pathon-shadow` is applied separately by the colour pass (the `shadows` flag), and this hook multiplies
// with it. Without a replacement every light is unshadowed here.
fn shadow_factor(light_index: u32, position: vec3<f32>) -> f32 {
    return 1.0;
}
