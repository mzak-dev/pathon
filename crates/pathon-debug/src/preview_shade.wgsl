@fragment
fn fs_preview(input: FullscreenOutput) -> @location(0) vec4<f32> {
    let texel = fetch(input.uv);
    var display = vec3<f32>(0.0);
    switch params.mode {
        case MODE_SCALAR: {
            display = vec3<f32>(clamp(texel.r, 0.0, 1.0));
        }
        case MODE_LINEAR_COLOR: {
            display = srgb_oetf(clamp(texel.rgb, vec3<f32>(0.0), vec3<f32>(1.0)));
        }
        case MODE_EXPOSED_HDR: {
            let exposed = texel.rgb * exposure_multiplier();
            if (all_finite(exposed)) {
                display = srgb_oetf(clamp(exposed, vec3<f32>(0.0), vec3<f32>(1.0)));
            } else {
                display = vec3<f32>(1.0, 0.0, 1.0);
            }
        }
        case MODE_DEPTH: {
            display = depth_color(texel.r, input.uv);
        }
        case MODE_NORMALS: {
            display = oct_decode(texel.rg) * 0.5 + vec3<f32>(0.5);
        }
        case MODE_MOTION: {
            display = motion_color(texel.rg);
        }
        case MODE_MATERIAL_ID: {
            display = hash_color(fetch_id(input.uv));
        }
        default: {
            // The exposure multiplier on a log scale: sixteen stops over the ramp, 1.0 in the middle.
            display = heat(log2(max(texel.r, 1.0e-6)) / 16.0 + 0.5);
        }
    }
    return vec4<f32>(for_target(display), 1.0);
}
