@group(0) @binding(3) var sky_view_lut: texture_2d<f32>;

struct FullscreenOutput {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> FullscreenOutput {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var output: FullscreenOutput;
    output.position = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    return output;
}

fn unproject(ndc: vec2<f32>, depth: f32) -> vec3<f32> {
    let point = sky.inverse_view_projection * vec4<f32>(ndc, depth, 1.0);
    return point.xyz / point.w;
}

// Radiance of the sun disc (nits) seen along `direction`: capped, because binary16 cannot hold the real value.
fn sun_disc(direction: vec3<f32>) -> vec3<f32> {
    let sun = sky.sun_direction.xyz;
    let along = dot(direction, sun);
    let cap = sky.radii.w;
    if (along <= 0.0 || cap <= 0.0) {
        return vec3<f32>(0.0);
    }
    let angle = atan2(length(cross(direction, sun)), along);
    let radius = sky.sun_direction.w;
    let coverage = 1.0 - smoothstep(0.8 * radius, radius, angle);
    if (coverage <= 0.0) {
        return vec3<f32>(0.0);
    }
    let radiance = sky.sun_illuminance.xyz * transmittance_to_top(sky.radii.z, direction.y) / sky.sun_illuminance.w;
    return min(radiance, vec3<f32>(cap)) * coverage;
}

@fragment
fn fs_sky(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    if (!is_sky_pixel(vec2<i32>(position.xy))) {
        discard;
    }
    let ndc = vec2<f32>(position.x / sky.view_sizes.z * 2.0 - 1.0, 1.0 - position.y / sky.view_sizes.w * 2.0);
    // Two points of the pixel ray: depth 1 is the near plane of the reverse-Z projection, 0.5 lies further away.
    let direction = normalize(unproject(ndc, 0.5) - unproject(ndc, 1.0));

    let radius = sky.radii.z;
    let mu = direction.y;
    let on_ground = mu < horizon_cosine(radius);
    let uv = sky_view_params_to_uv(on_ground, mu, azimuth_cosine(sky.sun_direction.xyz, direction), radius);
    let clear = textureSampleLevel(sky_view_lut, lut_sampler, uv, 0.0).rgb * sky.sun_illuminance.xyz;
    let coverage = sky.overcast.w;
    let overcast = sky.overcast.xyz * ((1.0 + 2.0 * max(mu, 0.0)) / 3.0);
    var color = mix(clear, overcast, coverage);
    if (!on_ground) {
        color += sun_disc(direction) * (1.0 - coverage);
    }
    return vec4<f32>(clamp(color, vec3<f32>(0.0), vec3<f32>(65000.0)), 1.0);
}
