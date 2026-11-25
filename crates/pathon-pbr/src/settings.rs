/// The cluster grid the point and spot lights are assigned to: `tile_size` pixel tiles times `slices` exponential depth
/// slices between `near` and `far` (metres). Everything closer than the end of the first slice shares slice 0 and
/// everything beyond `far` shares the last slice.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ClusterSettings {
    pub tile_size: u32,
    pub slices: u32,
    pub near: f32,
    pub far: f32,
    /// Capacity of the shared list of light indices; clusters past it get shorter lists (`L0005`).
    pub max_light_indices: u32,
}

impl Default for ClusterSettings {
    fn default() -> Self {
        Self {
            tile_size: 16,
            slices: 24,
            near: 0.1,
            far: 100.0,
            max_light_indices: 1 << 20,
        }
    }
}

/// How the colour pass finds the lights of a pixel. `BruteForce` tests every light and exists to check the clusters.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum LightAssignment {
    #[default]
    Clustered,
    BruteForce,
}

/// Settings of `PbrPack`, read from `Shared` every frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PbrSettings {
    pub clusters: ClusterSettings,
    pub light_assignment: LightAssignment,
    /// Scale of the image-based lighting, to match the units of the environment image to the lights.
    pub environment_intensity: f32,
}

impl Default for PbrSettings {
    fn default() -> Self {
        Self {
            clusters: ClusterSettings::default(),
            light_assignment: LightAssignment::default(),
            environment_intensity: 1.0,
        }
    }
}

impl PbrSettings {
    /// The first thing wrong with the settings (`L0001`), if anything.
    pub fn problem(&self) -> Option<String> {
        let clusters = &self.clusters;
        if !(4..=256).contains(&clusters.tile_size) || !clusters.tile_size.is_power_of_two() {
            return Some(format!("tile size {} is not a power of two in 4..=256", clusters.tile_size));
        }
        if !(1..=64).contains(&clusters.slices) {
            return Some(format!("{} depth slices, expected 1..=64", clusters.slices));
        }
        if !(clusters.near.is_finite() && clusters.far.is_finite() && 0.0 < clusters.near && clusters.near < clusters.far)
        {
            return Some(format!("cluster depth range {}..{} must satisfy 0 < near < far", clusters.near, clusters.far));
        }
        if clusters.max_light_indices == 0 {
            return Some("the light index list cannot be empty".to_owned());
        }
        if !(self.environment_intensity.is_finite() && self.environment_intensity >= 0.0) {
            return Some("environment intensity must be finite and not negative".to_owned());
        }
        None
    }

    /// The settings with every invalid value replaced by its default, and the problem that was found.
    pub fn sanitized(self) -> (Self, Option<String>) {
        match self.problem() {
            Some(problem) => (Self::default(), Some(problem)),
            None => (self, None),
        }
    }
}
