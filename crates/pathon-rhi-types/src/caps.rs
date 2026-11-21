use bitflags::bitflags;

use crate::error::RhiError;
use crate::format::Format;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Backend {
    Vulkan,
    Dx12,
    Metal,
    WebGpu,
    Null,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum DeviceType {
    DiscreteGpu,
    IntegratedGpu,
    VirtualGpu,
    Cpu,
    Other,
}

bitflags! {
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
    pub struct Features: u64 {
        const TIMESTAMP_QUERY = 1 << 0;
        const TIMESTAMP_QUERY_INSIDE_PASSES = 1 << 1;
        const TEXTURE_BINDING_ARRAY = 1 << 2;
        const BUFFER_BINDING_ARRAY = 1 << 3;
        const NON_UNIFORM_INDEXING = 1 << 4;
        const PARTIALLY_BOUND_BINDING_ARRAY = 1 << 5;
        const MULTI_DRAW_INDIRECT_COUNT = 1 << 6;
        const INDIRECT_FIRST_INSTANCE = 1 << 7;
        const SHADER_INT64 = 1 << 8;
        const SHADER_INT64_ATOMIC_MIN_MAX = 1 << 9;
        const SHADER_F16 = 1 << 10;
        const SUBGROUP = 1 << 11;
        const TEXTURE_ATOMIC = 1 << 12;
        const TEXTURE_FORMAT_16BIT_NORM = 1 << 13;
        const FLOAT32_FILTERABLE = 1 << 14;
        const TEXTURE_COMPRESSION_BC = 1 << 15;
        const TEXTURE_COMPRESSION_ETC2 = 1 << 16;
        const TEXTURE_COMPRESSION_ASTC = 1 << 17;
        const PIPELINE_CACHE = 1 << 18;
        const DEPTH_CLIP_CONTROL = 1 << 19;
        const DEPTH32FLOAT_STENCIL8 = 1 << 20;
        const RAY_QUERY = 1 << 21;
        const MESH_SHADER = 1 << 22;
        const POLYGON_MODE_LINE = 1 << 23;
        const IMMEDIATES = 1 << 24;
    }
}

bitflags! {
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
    pub struct FormatCaps: u32 {
        const SAMPLED = 1 << 0;
        const FILTERABLE = 1 << 1;
        const STORAGE = 1 << 2;
        const RENDER_TARGET = 1 << 3;
        const BLENDABLE = 1 << 4;
        const MULTISAMPLE = 1 << 5;
        const RESOLVE = 1 << 6;
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Limits {
    pub max_texture_dimension_2d: u32,
    pub max_texture_array_layers: u32,
    pub max_bind_groups: u32,
    pub max_bindings_per_bind_group: u32,
    pub max_uniform_buffer_binding_size: u64,
    pub max_storage_buffer_binding_size: u64,
    pub max_buffer_size: u64,
    pub max_color_attachments: u32,
    pub max_compute_workgroup_size: [u32; 3],
    pub max_compute_invocations_per_workgroup: u32,
    pub max_compute_workgroups_per_dimension: u32,
    pub max_binding_array_elements_per_shader_stage: u32,
    pub max_binding_array_sampler_elements_per_shader_stage: u32,
    pub max_immediate_size: u32,
    pub min_uniform_buffer_offset_alignment: u32,
    pub min_storage_buffer_offset_alignment: u32,
}

impl Limits {
    // WebGPU defaults: the floor every adapter guarantees.
    pub const WEBGPU_DEFAULT: Self = Self {
        max_texture_dimension_2d: 8192,
        max_texture_array_layers: 256,
        max_bind_groups: 4,
        max_bindings_per_bind_group: 1000,
        max_uniform_buffer_binding_size: 64 << 10,
        max_storage_buffer_binding_size: 128 << 20,
        max_buffer_size: 256 << 20,
        max_color_attachments: 8,
        max_compute_workgroup_size: [256, 256, 64],
        max_compute_invocations_per_workgroup: 256,
        max_compute_workgroups_per_dimension: 65535,
        max_binding_array_elements_per_shader_stage: 0,
        max_binding_array_sampler_elements_per_shader_stage: 0,
        max_immediate_size: 0,
        min_uniform_buffer_offset_alignment: 256,
        min_storage_buffer_offset_alignment: 256,
    };
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AdapterInfo {
    pub name: String,
    pub driver: String,
    pub device_type: DeviceType,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Profile {
    Web,
    Desktop,
    High,
}

impl Profile {
    pub const ALL: [Self; 3] = [Self::Web, Self::Desktop, Self::High];

    pub fn features(self) -> Features {
        let desktop = Features::TEXTURE_BINDING_ARRAY
            | Features::NON_UNIFORM_INDEXING
            | Features::PARTIALLY_BOUND_BINDING_ARRAY
            | Features::INDIRECT_FIRST_INSTANCE;
        match self {
            Self::Web => Features::empty(),
            Self::Desktop => desktop,
            Self::High => desktop | Features::SHADER_INT64_ATOMIC_MIN_MAX,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Caps {
    pub backend: Backend,
    pub adapter: AdapterInfo,
    pub features: Features,
    pub limits: Limits,
    pub formats: Vec<(Format, FormatCaps)>,
    pub timestamp_period_ns: f32,
}

impl Caps {
    pub fn has(&self, features: Features) -> bool {
        self.features.contains(features)
    }

    pub fn missing(&self, features: Features) -> Features {
        features.difference(self.features)
    }

    pub fn format(&self, format: Format) -> FormatCaps {
        self.formats
            .iter()
            .find(|(candidate, _)| *candidate == format)
            .map_or(FormatCaps::empty(), |(_, caps)| *caps)
    }

    pub fn check_immediate_size(&self, label: &str, immediate_size: u32) -> Result<(), RhiError> {
        if immediate_size == 0 {
            return Ok(());
        }
        if !self.has(Features::IMMEDIATES) {
            return Err(RhiError::MissingFeatures(Features::IMMEDIATES));
        }
        let max = self.limits.max_immediate_size;
        if immediate_size > max || !immediate_size.is_multiple_of(4) {
            return Err(RhiError::validation(
                "R0107",
                format!(
                    "pipeline '{label}': {immediate_size} bytes of immediates must be a multiple of 4 and at most {max}"
                ),
            ));
        }
        Ok(())
    }

    pub fn satisfies(&self, profile: Profile) -> bool {
        self.has(profile.features())
    }

    pub fn best_profile(&self) -> Profile {
        let mut best = Profile::Web;
        for profile in Profile::ALL {
            if self.satisfies(profile) {
                best = profile;
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps_with(features: Features) -> Caps {
        Caps {
            backend: Backend::Null,
            adapter: AdapterInfo {
                name: "test".into(),
                driver: String::new(),
                device_type: DeviceType::Cpu,
            },
            features,
            limits: Limits::WEBGPU_DEFAULT,
            formats: vec![(Format::Rgba8Unorm, FormatCaps::SAMPLED)],
            timestamp_period_ns: 1.0,
        }
    }

    #[test]
    fn profiles_are_nested() {
        assert!(Profile::High.features().contains(Profile::Desktop.features()));
        assert!(Profile::Desktop.features().contains(Profile::Web.features()));
    }

    #[test]
    fn best_profile_follows_feature_bits() {
        assert_eq!(caps_with(Features::empty()).best_profile(), Profile::Web);
        assert_eq!(caps_with(Profile::Desktop.features()).best_profile(), Profile::Desktop);
        assert_eq!(caps_with(Profile::High.features()).best_profile(), Profile::High);
    }

    #[test]
    fn unknown_format_has_no_caps() {
        let caps = caps_with(Features::empty());
        assert_eq!(caps.format(Format::Bc7RgbaUnorm), FormatCaps::empty());
        assert_eq!(caps.format(Format::Rgba8Unorm), FormatCaps::SAMPLED);
    }
}
