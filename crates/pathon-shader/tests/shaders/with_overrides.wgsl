override intensity: f32 = 1.0;
@id(7) override samples: u32 = 4u;

@compute @workgroup_size(1)
fn main() {
    let scaled = intensity * f32(samples);
}
