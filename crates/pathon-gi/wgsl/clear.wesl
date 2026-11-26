// Empties the voxel grid before the scene is voxelized again.
import package::gi::common::GiVolume;

// GI_BINDINGS

@compute @workgroup_size(64)
fn clear_voxels(@builtin(global_invocation_id) id: vec3<u32>) {
    let cell = id.x;
    if (cell == 0u) {
        atomicStore(&solid_cells[0], 0u);
    }
    if (cell >= gi_grid.dims.w) {
        return;
    }
    voxel_albedo[cell] = 0u;
    voxel_emissive[cell] = 0u;
    for (var face = 0u; face < 6u; face++) {
        voxel_radiance[cell * 6u + face] = 0u;
    }
}
