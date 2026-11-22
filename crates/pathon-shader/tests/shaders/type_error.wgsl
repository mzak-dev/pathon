// A float is stored in an unsigned integer variable on line 4.
@compute @workgroup_size(1)
fn main() {
    var count: u32 = 1.5;
}
