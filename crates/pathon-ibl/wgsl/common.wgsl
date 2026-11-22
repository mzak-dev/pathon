const PI: f32 = 3.14159265358979;
// Largest finite binary16 value with headroom: a store beyond it would become infinity and poison every filter.
const MAX_RADIANCE: f32 = 60000.0;

// Direction through the centre of a point (s, t) in -1..1 on a cube face; faces are +X, -X, +Y, -Y, +Z, -Z and
// t grows downwards in the texture, as in WebGPU, Vulkan and Metal.
fn face_direction(face: u32, s: f32, t: f32) -> vec3<f32> {
    switch face {
        case 0u: { return normalize(vec3<f32>(1.0, -t, -s)); }
        case 1u: { return normalize(vec3<f32>(-1.0, -t, s)); }
        case 2u: { return normalize(vec3<f32>(s, 1.0, t)); }
        case 3u: { return normalize(vec3<f32>(s, -1.0, -t)); }
        case 4u: { return normalize(vec3<f32>(s, -t, 1.0)); }
        default: { return normalize(vec3<f32>(-s, -t, -1.0)); }
    }
}

// `x` and `y` are continuous texel coordinates, so a texel centre is (i + 0.5).
fn texel_direction(face: u32, x: f32, y: f32, size: f32) -> vec3<f32> {
    return face_direction(face, 2.0 * x / size - 1.0, 2.0 * y / size - 1.0);
}

fn hammersley(index: u32, count: u32) -> vec2<f32> {
    return vec2<f32>(f32(index) / f32(count), f32(reverseBits(index)) * 2.3283064365386963e-10);
}

fn tangent_frame(n: vec3<f32>) -> mat3x3<f32> {
    let up = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(n.z) < 0.999);
    let t = normalize(cross(up, n));
    return mat3x3<f32>(t, cross(n, t), n);
}

// Half vector in a frame whose z axis is the normal, distributed as D(h) n.h.
fn importance_sample_ggx(xi: vec2<f32>, alpha: f32) -> vec3<f32> {
    let phi = 2.0 * PI * xi.x;
    let cos_theta = sqrt((1.0 - xi.y) / (1.0 + (alpha * alpha - 1.0) * xi.y));
    let sin_theta = sqrt(max(1.0 - cos_theta * cos_theta, 0.0));
    return vec3<f32>(sin_theta * cos(phi), sin_theta * sin(phi), cos_theta);
}

fn distribution_ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let f = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * f * f);
}

// Height-correlated Smith visibility, already divided by 4 n.l n.v: the same function `material::evaluate` uses.
fn visibility_smith_ggx(n_dot_l: f32, n_dot_v: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let ggx_v = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
    let ggx_l = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
    let sum = ggx_v + ggx_l;
    return select(0.0, 0.5 / sum, sum > 0.0);
}
