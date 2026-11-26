// One thread per ray of the probes updated this frame: marches the voxels from the probe and records what the ray
// saw: the radiance of the face it hit, or the environment when it left the grid.
import package::gi::common::{GiVolume, probe_cell, probe_position, spherical_fibonacci, unpack_rgb9e5};

// GI_BINDINGS
// GI_VOXELS

const RAY_HIT: f32 = 0.0;
const RAY_SKY: f32 = 1.0;
const RAY_INSIDE: f32 = 2.0;

@compute @workgroup_size(64)
fn trace_probes(@builtin(global_invocation_id) id: vec3<u32>) {
    let rays = gi_pass.frame.w;
    let batch = gi_pass.frame.z;
    let global_ray = id.x;
    if (global_ray >= batch * rays) {
        return;
    }
    let slot = global_ray / rays;
    let ray = global_ray % rays;
    let probes = gi_volume.counts.x * gi_volume.counts.y * gi_volume.counts.z;
    let probe = (gi_pass.frame.y + slot) % probes;
    let position = probe_position(gi_volume, probe_cell(gi_volume, probe));

    let base = spherical_fibonacci(ray, rays);
    let direction = normalize(vec3<f32>(
        dot(gi_pass.rotation_x.xyz, base),
        dot(gi_pass.rotation_y.xyz, base),
        dot(gi_pass.rotation_z.xyz, base),
    ));
    let first = global_ray * 2u;
    if (cell_solid(world_cell(position))) {
        gi_rays[first] = vec4<f32>(0.0);
        gi_rays[first + 1u] = vec4<f32>(direction, RAY_INSIDE);
        return;
    }
    let reach = gi_volume.bias.z;
    let hit = trace_voxels(position, direction, reach);
    if (hit.hit) {
        let radiance = unpack_rgb9e5(voxel_radiance[cell_index(hit.cell) * 6u + hit.face]);
        gi_rays[first] = vec4<f32>(radiance, hit.distance);
        gi_rays[first + 1u] = vec4<f32>(direction, RAY_HIT);
        return;
    }
    let levels = f32(textureNumLevels(environment) - 1u);
    let sky = textureSampleLevel(environment, environment_sampler, direction, levels * 0.3).rgb * gi_pass.blend.y;
    gi_rays[first] = vec4<f32>(sky, reach);
    gi_rays[first + 1u] = vec4<f32>(direction, RAY_SKY);
}
