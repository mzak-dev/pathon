// A module whose function stores a u32 in an f32 on line 7.
fn offset() -> u32 {
    return 2u;
}

fn shade(level: f32) -> f32 {
    let shifted: f32 = level + offset();
    return shifted;
}
