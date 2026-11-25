struct Params {
    min_log2: f32,
    log2_range: f32,
    low_fraction: f32,
    high_fraction: f32,
    key: f32,
    manual_multiplier: f32,
    brighter_coefficient: f32,
    darker_coefficient: f32,
    previous_valid: u32,
    padding0: u32,
    padding1: u32,
    padding2: u32,
}

const BINS: u32 = 256u;

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var hdr: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> histogram: array<atomic<u32>, 256>;
@group(0) @binding(3) var previous_exposure: texture_2d<f32>;
@group(0) @binding(4) var exposure_out: texture_storage_2d<r32float, write>;

var<workgroup> local_histogram: array<atomic<u32>, 256>;

// Bin 0 takes everything darker than the range (black included), 1..255 split the range evenly.
fn bin_of(value: f32) -> u32 {
    if (value <= 0.0) {
        return 0u;
    }
    let position = (log2(value) - params.min_log2) / params.log2_range;
    if (position < 0.0) {
        return 0u;
    }
    return 1u + min(u32(position * f32(BINS - 1u)), BINS - 2u);
}

@compute @workgroup_size(256)
fn clear(@builtin(local_invocation_index) index: u32) {
    atomicStore(&histogram[index], 0u);
}

// Counts per workgroup first: a flat grey frame would otherwise put every pixel on one global atomic.
@compute @workgroup_size(16, 16)
fn build(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) index: u32) {
    atomicStore(&local_histogram[index], 0u);
    workgroupBarrier();
    let size = textureDimensions(hdr);
    if (id.x < size.x && id.y < size.y) {
        let color = sanitize(textureLoad(hdr, vec2<i32>(id.xy), 0).rgb);
        atomicAdd(&local_histogram[bin_of(luminance(color))], 1u);
    }
    workgroupBarrier();
    let count = atomicLoad(&local_histogram[index]);
    if (count > 0u) {
        atomicAdd(&histogram[index], count);
    }
}

@compute @workgroup_size(1)
fn reduce() {
    var total = 0.0;
    for (var bin = 1u; bin < BINS; bin++) {
        total += f32(atomicLoad(&histogram[bin]));
    }
    let low = total * params.low_fraction;
    let high = total * params.high_fraction;
    var cumulative = 0.0;
    var weighted = 0.0;
    var weight = 0.0;
    for (var bin = 1u; bin < BINS; bin++) {
        let start = cumulative;
        cumulative += f32(atomicLoad(&histogram[bin]));
        let inside = max(min(cumulative, high) - max(start, low), 0.0);
        let center = params.min_log2 + (f32(bin - 1u) + 0.5) / f32(BINS - 1u) * params.log2_range;
        weighted += inside * center;
        weight += inside;
    }
    // A black frame has nothing to measure: the darkest end of the range gives the strongest exposure.
    var average_log2 = params.min_log2;
    if (weight > 0.0) {
        average_log2 = weighted / weight;
    }
    let target_log2 = log2(params.key) - average_log2;

    var log2_exposure = target_log2;
    let previous = textureLoad(previous_exposure, vec2<i32>(0, 0), 0).r;
    if (params.previous_valid != 0u && previous > 0.0 && previous < 1.0e30) {
        let previous_log2 = log2(previous);
        let coefficient = select(params.darker_coefficient, params.brighter_coefficient, target_log2 < previous_log2);
        log2_exposure = previous_log2 + (target_log2 - previous_log2) * coefficient;
    }
    textureStore(exposure_out, vec2<i32>(0, 0), vec4<f32>(exp2(log2_exposure), 0.0, 0.0, 0.0));
}

@compute @workgroup_size(1)
fn manual() {
    textureStore(exposure_out, vec2<i32>(0, 0), vec4<f32>(params.manual_multiplier, 0.0, 0.0, 0.0));
}
