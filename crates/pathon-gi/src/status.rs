/// What the feature did in the last frame it recorded, kept in `Shared` for logs and tools (the benchmark of the viewer
/// prints it next to the pass times).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct GiStatus {
    pub probes: u32,
    pub probe_spacing: f32,
    pub voxel_cells: u32,
    pub voxel_size: f32,
    pub rays_per_frame: u32,
    /// Draws of the voxelizer: instances (large meshes count more than once).
    pub voxel_jobs: u32,
    /// The scene was voxelized again this frame.
    pub revoxelized: bool,
    /// GPU memory of the volume in bytes.
    pub gpu_bytes: u64,
}

impl std::fmt::Display for GiStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} probes every {:.2} m, {} rays per frame, {} voxels of {:.2} m from {} draws, {:.1} MiB",
            self.probes,
            self.probe_spacing,
            self.rays_per_frame,
            self.voxel_cells,
            self.voxel_size,
            self.voxel_jobs,
            self.gpu_bytes as f64 / (1024.0 * 1024.0),
        )
    }
}
