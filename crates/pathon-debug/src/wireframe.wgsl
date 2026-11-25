// Triangle soup: every triangle has its own three vertices, so the corner of a vertex is its index modulo three.

struct WireInput {
    @builtin(vertex_index) index: u32,
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) thickness: f32,
}

struct WireOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) barycentric: vec3<f32>,
    @location(2) @interpolate(flat) thickness: f32,
}

@vertex
fn vs_wire(input: WireInput) -> WireOutput {
    var corners = array<vec3<f32>, 3>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
    var output: WireOutput;
    output.position = frame.view_projection * vec4<f32>(input.position, 1.0);
    output.color = input.color;
    output.barycentric = corners[input.index % 3u];
    output.thickness = input.thickness;
    return output;
}

// The pipeline rasterizes the edges itself (polygon mode line): one pixel wide, no antialiasing.
@fragment
fn fs_hardware(input: WireOutput) -> @location(0) vec4<f32> {
    if (occluded(input.position)) {
        discard;
    }
    return premultiplied(input.color.rgb, input.color.a);
}

// Fills the triangles and keeps the pixels near an edge. Only the inside of the triangle is rasterized, so a border
// edge is drawn half as wide as an edge shared by two triangles.
@fragment
fn fs_barycentric(input: WireOutput) -> @location(0) vec4<f32> {
    let pixels_per_unit = max(fwidth(input.barycentric), vec3<f32>(1.0e-6));
    let to_edge = input.barycentric / pixels_per_unit;
    let nearest = min(to_edge.x, min(to_edge.y, to_edge.z));
    let coverage = clamp(input.thickness * 0.5 + 0.5 - nearest, 0.0, 1.0);
    if (coverage <= 0.0 || occluded(input.position)) {
        discard;
    }
    return premultiplied(input.color.rgb, input.color.a * coverage);
}
