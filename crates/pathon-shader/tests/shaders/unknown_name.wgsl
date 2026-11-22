// `intensty` on line 5 is a typo for `intensity`.
@compute @workgroup_size(1)
fn main() {
    let intensity = 1.0;
    let lit = intensty * 2.0;
}
