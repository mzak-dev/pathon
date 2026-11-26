// Probe volume layout, octahedral directions and the packed colour of the voxels, shared by the sampling module and the
// compute passes of `pathon-gi`.

const PI: f32 = 3.14159265358979;

// Interior texels of a probe tile and the tile with its one-texel border (the border repeats the wrapped interior, so a
// hardware bilinear fetch stays inside the probe).
const IRRADIANCE_RES: u32 = 8u;
const IRRADIANCE_TILE: u32 = 10u;
const VISIBILITY_RES: u32 = 16u;
const VISIBILITY_TILE: u32 = 18u;

// 80 bytes, mirrored by `volume::GiVolumeUniforms`.
struct GiVolume {
    // xyz: world position of probe (0, 0, 0), w: distance between probes in metres.
    origin_spacing: vec4<f32>,
    // Probes along x, y and z, probes per atlas row.
    counts: vec4<u32>,
    // Irradiance atlas size, then visibility atlas size, in texels.
    atlas: vec4<f32>,
    // Normal bias and view bias in metres, longest ray in metres, scale of the indirect light.
    bias: vec4<f32>,
    // x: strength of the sky visibility on specular light, y: cells over which the volume fades out at its edge.
    tuning: vec4<f32>,
}

fn sign_not_zero(v: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(select(-1.0, 1.0, v.x >= 0.0), select(-1.0, 1.0, v.y >= 0.0));
}

// Octahedral mapping of a unit vector to [-1, 1]^2 (Cigolle et al. 2014).
fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    if (n.z < 0.0) {
        return (vec2<f32>(1.0) - abs(p.yx)) * sign_not_zero(p);
    }
    return p;
}

fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    if (n.z < 0.0) {
        let folded = (vec2<f32>(1.0) - abs(n.yx)) * sign_not_zero(n.xy);
        n = vec3<f32>(folded.x, folded.y, n.z);
    }
    return normalize(n);
}

fn probe_tile(volume: GiVolume, index: u32) -> vec2<u32> {
    let per_row = volume.counts.w;
    return vec2<u32>(index % per_row, index / per_row);
}

fn probe_cell(volume: GiVolume, index: u32) -> vec3<u32> {
    let nx = volume.counts.x;
    let ny = volume.counts.y;
    return vec3<u32>(index % nx, (index / nx) % ny, index / (nx * ny));
}

fn probe_index(volume: GiVolume, cell: vec3<u32>) -> u32 {
    return cell.x + volume.counts.x * (cell.y + volume.counts.y * cell.z);
}

fn probe_position(volume: GiVolume, cell: vec3<u32>) -> vec3<f32> {
    return volume.origin_spacing.xyz + vec3<f32>(cell) * volume.origin_spacing.w;
}

// Spherical Fibonacci points (Keinert et al. 2015): `index` of `count` nearly evenly spread unit vectors.
fn spherical_fibonacci(index: u32, count: u32) -> vec3<f32> {
    let phi = 2.0 * PI * fract(f32(index) * 0.61803398875);
    let cos_theta = 1.0 - (2.0 * f32(index) + 1.0) / f32(count);
    let sin_theta = sqrt(saturate(1.0 - cos_theta * cos_theta));
    return vec3<f32>(cos(phi) * sin_theta, sin(phi) * sin_theta, cos_theta);
}

// RGB with a shared 5-bit exponent and 9-bit mantissas (the layout of `EXT_texture_shared_exponent`): four bytes for a
// radiance that spans several orders of magnitude.
fn pack_rgb9e5(color: vec3<f32>) -> u32 {
    let clamped = clamp(color, vec3<f32>(0.0), vec3<f32>(65408.0));
    let largest = max(clamped.x, max(clamped.y, clamped.z));
    if (largest < 1.0e-9) {
        return 0u;
    }
    var exponent = max(-16, i32(floor(log2(largest)))) + 16;
    var scale = exp2(f32(exponent - 24));
    if (u32(floor(largest / scale + 0.5)) == 512u) {
        exponent = exponent + 1;
        scale = scale * 2.0;
    }
    let mantissa = vec3<u32>(floor(clamped / scale + 0.5));
    return (u32(exponent) << 27u) | (mantissa.z << 18u) | (mantissa.y << 9u) | mantissa.x;
}

fn unpack_rgb9e5(packed: u32) -> vec3<f32> {
    let exponent = i32(packed >> 27u);
    let scale = exp2(f32(exponent - 24));
    let mantissa = vec3<u32>(packed & 511u, (packed >> 9u) & 511u, (packed >> 18u) & 511u);
    return vec3<f32>(mantissa) * scale;
}
