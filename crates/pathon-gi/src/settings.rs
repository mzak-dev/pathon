use pathon_scene::Aabb;

/// Most cells a voxel grid may have: the passes that visit every cell dispatch one thread each and a dispatch has at
/// most 65535 groups of 64.
pub const MAX_VOXELS: u32 = 4_000_000;
/// Most rays a frame traces in all (`probes_per_frame * rays_per_probe`), for the same reason.
pub const MAX_RAYS_PER_FRAME: u32 = 4_000_000;

/// Settings of `GiPack`, read from `Shared` every frame. A change of `bounds`, `probe_spacing`, `max_probes`,
/// `voxel_size`, `max_voxels` or `rays_per_probe` rebuilds the volume and starts the probes again; the rest takes
/// effect on the next frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GiSettings {
    /// Off: no passes run and nothing is published, so lighting falls back to the environment.
    pub enabled: bool,
    /// The box the probes and the voxels cover; `None` takes the bounds of the voxelized instances.
    pub bounds: Option<Aabb>,
    /// Distance between probes in metres. Grown when `max_probes` would be exceeded.
    pub probe_spacing: f32,
    pub max_probes: u32,
    /// Edge of a voxel in metres: the smallest gap a ray can see through. Grown when `max_voxels` would be exceeded.
    pub voxel_size: f32,
    pub max_voxels: u32,
    pub rays_per_probe: u32,
    /// Probes updated per frame; the rest keep their value. The batch moves round-robin.
    pub probes_per_frame: u32,
    /// Weight of the previous value when a probe is updated (0 = take the new rays only, 1 = never change).
    pub hysteresis: f32,
    /// Exponent of the cosine weight of the distance moments: a larger one resolves finer occluders and needs more
    /// rays.
    pub depth_sharpness: f32,
    /// Longest ray in metres: what is farther is the environment, and the visibility moments saturate there.
    pub max_ray_distance: f32,
    /// Offsets of the lookup point along the normal and towards the eye, in metres, against the light of the wall
    /// a probe sits behind.
    pub normal_bias: f32,
    pub view_bias: f32,
    /// A cell is lit again every this many frames (1 = every frame).
    pub voxel_relight_period: u32,
    /// Scale of the light the voxels get from the probes, i.e. of the second and later bounces (0 = one bounce).
    pub bounce_intensity: f32,
    /// Scale of the indirect light the surfaces receive.
    pub indirect_intensity: f32,
    /// Scale of the environment as the probes see it; set it to `PbrSettings::environment_intensity`.
    pub environment_intensity: f32,
    /// How much of the sky visibility the probes measured darkens the specular environment (0 = none).
    pub specular_occlusion: f32,
}

impl Default for GiSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            bounds: None,
            probe_spacing: 0.5,
            max_probes: 4096,
            voxel_size: 0.1,
            max_voxels: 3_000_000,
            rays_per_probe: 128,
            probes_per_frame: u32::MAX,
            hysteresis: 0.95,
            depth_sharpness: 12.0,
            max_ray_distance: 40.0,
            normal_bias: 0.1,
            view_bias: 0.05,
            voxel_relight_period: 4,
            bounce_intensity: 1.0,
            indirect_intensity: 1.0,
            environment_intensity: 1.0,
            specular_occlusion: 1.0,
        }
    }
}

impl GiSettings {
    /// A small, quick volume for the Web and weak GPUs: 1 m probes, 64 rays, 20 cm voxels.
    pub const COMPACT: Self = Self {
        enabled: true,
        bounds: None,
        probe_spacing: 1.0,
        max_probes: 1024,
        voxel_size: 0.2,
        max_voxels: 600_000,
        rays_per_probe: 64,
        probes_per_frame: u32::MAX,
        hysteresis: 0.95,
        depth_sharpness: 8.0,
        max_ray_distance: 40.0,
        normal_bias: 0.15,
        view_bias: 0.08,
        voxel_relight_period: 8,
        bounce_intensity: 1.0,
        indirect_intensity: 1.0,
        environment_intensity: 1.0,
        specular_occlusion: 1.0,
    };

    /// The first thing wrong with the settings (`J0001`), if anything.
    pub fn problem(&self) -> Option<String> {
        let positive = |name: &str, value: f32| {
            (!(value.is_finite() && value > 0.0)).then(|| format!("{name} must be finite and positive, got {value}"))
        };
        let not_negative = |name: &str, value: f32| {
            (!(value.is_finite() && value >= 0.0))
                .then(|| format!("{name} must be finite and not negative, got {value}"))
        };
        if let Some(bounds) = self.bounds
            && !(bounds.is_bounded() && bounds.min.cmplt(bounds.max).all())
        {
            return Some("bounds must be a finite box with a positive size".to_owned());
        }
        if !(1..=256).contains(&(self.rays_per_probe)) {
            return Some(format!("{} rays per probe, expected 1..=256", self.rays_per_probe));
        }
        if self.max_probes == 0 || self.probes_per_frame == 0 {
            return Some("max_probes and probes_per_frame must not be zero".to_owned());
        }
        if !(1..=MAX_VOXELS).contains(&self.max_voxels) {
            return Some(format!("max_voxels {} is outside 1..={MAX_VOXELS}", self.max_voxels));
        }
        if self.voxel_relight_period == 0 {
            return Some("voxel_relight_period must not be zero".to_owned());
        }
        if !(self.hysteresis.is_finite() && (0.0..1.0).contains(&self.hysteresis)) {
            return Some(format!("hysteresis {} is outside 0..1", self.hysteresis));
        }
        [
            positive("probe_spacing", self.probe_spacing),
            positive("voxel_size", self.voxel_size),
            positive("depth_sharpness", self.depth_sharpness),
            positive("max_ray_distance", self.max_ray_distance),
            not_negative("normal_bias", self.normal_bias),
            not_negative("view_bias", self.view_bias),
            not_negative("bounce_intensity", self.bounce_intensity),
            not_negative("indirect_intensity", self.indirect_intensity),
            not_negative("environment_intensity", self.environment_intensity),
            not_negative("specular_occlusion", self.specular_occlusion),
        ]
        .into_iter()
        .flatten()
        .next()
    }

    /// The settings with every invalid value replaced by its default, and the problem that was found.
    pub fn sanitized(self) -> (Self, Option<String>) {
        match self.problem() {
            Some(problem) => (Self::default(), Some(problem)),
            None => (self, None),
        }
    }

    /// Whether `other` needs another volume (and new probes) rather than another frame of the same one.
    #[expect(
        clippy::float_cmp,
        reason = "any change of these settings, however small, builds another volume"
    )]
    pub fn same_volume(&self, other: &Self) -> bool {
        self.bounds == other.bounds
            && self.probe_spacing == other.probe_spacing
            && self.max_probes == other.max_probes
            && self.voxel_size == other.voxel_size
            && self.max_voxels == other.max_voxels
            && self.rays_per_probe == other.rays_per_probe
            && self.probes_per_frame.min(self.max_probes) == other.probes_per_frame.min(other.max_probes)
    }
}

#[cfg(test)]
mod tests {
    use pathon_scene::Aabb;

    use super::*;

    #[test]
    fn the_defaults_are_valid() {
        assert_eq!(GiSettings::default().problem(), None);
        assert_eq!(GiSettings::COMPACT.problem(), None);
    }

    #[test]
    fn a_bad_value_is_named_and_replaced_by_the_defaults() {
        let bad = GiSettings {
            hysteresis: 1.5,
            ..GiSettings::default()
        };

        let (settings, problem) = bad.sanitized();

        assert_eq!(settings, GiSettings::default());
        assert!(problem.expect("a problem").contains("hysteresis"));
    }

    #[test]
    fn nan_is_not_accepted() {
        for settings in [
            GiSettings {
                probe_spacing: f32::NAN,
                ..GiSettings::default()
            },
            GiSettings {
                max_ray_distance: f32::INFINITY,
                ..GiSettings::default()
            },
            GiSettings {
                bounds: Some(Aabb::UNBOUNDED),
                ..GiSettings::default()
            },
        ] {
            assert!(settings.problem().is_some(), "{settings:?}");
        }
    }

    #[test]
    fn only_volume_fields_make_another_volume() {
        let base = GiSettings::default();

        assert!(base.same_volume(&GiSettings {
            hysteresis: 0.5,
            ..base
        }));
        assert!(!base.same_volume(&GiSettings {
            probe_spacing: 1.0,
            ..base
        }));
    }
}
