// One instance per segment, six vertices per instance: a quad in screen space around the segment, wide enough for
// the line, its round caps and one pixel of antialiasing. The fragment shader measures the distance to the segment.

struct LineInput {
    @builtin(vertex_index) corner: u32,
    @location(0) start: vec4<f32>,
    @location(1) end: vec3<f32>,
    @location(2) flags: u32,
    @location(3) color: vec4<f32>,
}

struct LineOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) segment: vec4<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) half_width: f32,
    @location(3) @interpolate(flat) depth_test: u32,
}

const NEAR_W: f32 = 1.0e-4;
const FLAG_DEPTH_TEST: u32 = 1u;

fn to_pixels(ndc: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * frame.viewport;
}

fn to_ndc(pixel: vec2<f32>) -> vec2<f32> {
    let uv = pixel / frame.viewport;
    return vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
}

@vertex
fn vs_line(input: LineInput) -> LineOutput {
    var output: LineOutput;
    // Outside the clip volume: a segment that is entirely behind the camera draws nothing.
    output.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    var a = frame.view_projection * vec4<f32>(input.start.xyz, 1.0);
    var b = frame.view_projection * vec4<f32>(input.end, 1.0);
    if (a.w < NEAR_W && b.w < NEAR_W) {
        return output;
    }
    if (a.w < NEAR_W) {
        a = mix(a, b, (NEAR_W - a.w) / (b.w - a.w));
    } else if (b.w < NEAR_W) {
        b = mix(b, a, (NEAR_W - b.w) / (a.w - b.w));
    }
    let ndc_a = a.xyz / a.w;
    let ndc_b = b.xyz / b.w;
    let pixel_a = to_pixels(ndc_a.xy);
    let pixel_b = to_pixels(ndc_b.xy);

    var direction = pixel_b - pixel_a;
    let length_pixels = length(direction);
    if (length_pixels > 1.0e-4) {
        direction = direction / length_pixels;
    } else {
        direction = vec2<f32>(1.0, 0.0);
    }
    let normal = vec2<f32>(-direction.y, direction.x);
    let half_width = input.start.w * 0.5;
    let extent = half_width + 1.0;

    var along = array<f32, 6>(0.0, 1.0, 0.0, 0.0, 1.0, 1.0);
    var side = array<f32, 6>(-1.0, -1.0, 1.0, 1.0, -1.0, 1.0);
    let t = along[input.corner];
    let pixel = mix(pixel_a, pixel_b, t) + direction * ((t * 2.0 - 1.0) * extent) + normal * (side[input.corner] * extent);

    output.position = vec4<f32>(to_ndc(pixel), clamp(mix(ndc_a.z, ndc_b.z, t), 0.0, 1.0), 1.0);
    output.segment = vec4<f32>(pixel_a, pixel_b);
    output.color = input.color;
    output.half_width = half_width;
    output.depth_test = input.flags & FLAG_DEPTH_TEST;
    return output;
}

@fragment
fn fs_line(input: LineOutput) -> @location(0) vec4<f32> {
    let a = input.segment.xy;
    let b = input.segment.zw;
    let ab = b - a;
    let length_squared = dot(ab, ab);
    var t = 0.0;
    if (length_squared > 1.0e-8) {
        t = clamp(dot(input.position.xy - a, ab) / length_squared, 0.0, 1.0);
    }
    let distance_pixels = length(input.position.xy - (a + ab * t));
    let coverage = clamp(input.half_width + 0.5 - distance_pixels, 0.0, 1.0);
    if (coverage <= 0.0 || (input.depth_test != 0u && occluded(input.position))) {
        discard;
    }
    return premultiplied(input.color.rgb, input.color.a * coverage);
}
