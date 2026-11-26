// Lists the solid cells, so that the lighting of the voxels runs one thread per face of a cell that is there, not one
// per cell of the grid (the grid is mostly air).
import package::gi::common::GiVolume;

// GI_BINDINGS

@compute @workgroup_size(64)
fn compact_solid(@builtin(global_invocation_id) id: vec3<u32>) {
    let cell = id.x;
    if (cell >= gi_grid.dims.w || (voxel_albedo[cell] >> 24u) == 0u) {
        return;
    }
    let slot = atomicAdd(&solid_cells[0], 1u);
    atomicStore(&solid_cells[1u + slot], cell);
}
