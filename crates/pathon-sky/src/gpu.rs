use pathon_api::resources::{UniformRing, UniformSlice};
use pathon_api::rhi::{
    BindGroupLayoutEntry, BindingType, DeviceApi, Format, RhiError, SamplerBindingType, ShaderStages, StorageAccess,
    TextureSampleType, TextureViewDimension,
};

pub(crate) const LUT_FORMAT: Format = Format::Rgba16Float;

const ATMOSPHERE_WGSL: &str = include_str!("../wgsl/atmosphere.wgsl");
const TRANSMITTANCE_LOOKUP_WGSL: &str = include_str!("../wgsl/transmittance_lookup.wgsl");
const MULTISCATTER_LOOKUP_WGSL: &str = include_str!("../wgsl/multiscatter_lookup.wgsl");
const SKYVIEW_MAPPING_WGSL: &str = include_str!("../wgsl/skyview_mapping.wgsl");
const TRANSMITTANCE_WGSL: &str = include_str!("../wgsl/transmittance.wgsl");
const MULTISCATTER_WGSL: &str = include_str!("../wgsl/multiscatter.wgsl");
const SKYVIEW_WGSL: &str = include_str!("../wgsl/skyview.wgsl");
const SKY_WGSL: &str = include_str!("../wgsl/sky.wgsl");
const DEPTH_TEST_WGSL: &str = include_str!("../wgsl/depth_test.wgsl");
const NO_DEPTH_WGSL: &str = include_str!("../wgsl/no_depth.wgsl");

// A WGSL module is a concatenation, because two passes may declare different resources at the same binding.
fn joined(parts: &[&str]) -> String {
    parts.join("\n")
}

pub(crate) fn transmittance_wgsl() -> String {
    joined(&[ATMOSPHERE_WGSL, TRANSMITTANCE_WGSL])
}

pub(crate) fn multiscatter_wgsl() -> String {
    joined(&[ATMOSPHERE_WGSL, TRANSMITTANCE_LOOKUP_WGSL, MULTISCATTER_WGSL])
}

pub(crate) fn sky_view_wgsl() -> String {
    joined(&[
        ATMOSPHERE_WGSL,
        TRANSMITTANCE_LOOKUP_WGSL,
        MULTISCATTER_LOOKUP_WGSL,
        SKYVIEW_MAPPING_WGSL,
        SKYVIEW_WGSL,
    ])
}

pub(crate) fn sky_wgsl(with_depth: bool) -> String {
    joined(&[
        ATMOSPHERE_WGSL,
        TRANSMITTANCE_LOOKUP_WGSL,
        SKYVIEW_MAPPING_WGSL,
        SKY_WGSL,
        if with_depth { DEPTH_TEST_WGSL } else { NO_DEPTH_WGSL },
    ])
}

pub(crate) const fn uniform_entry(visibility: ShaderStages) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding: 0,
        visibility,
        ty: BindingType::UniformBuffer,
        count: None,
    }
}

pub(crate) const fn texture_entry(
    binding: u32,
    visibility: ShaderStages,
    sample_type: TextureSampleType,
) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility,
        ty: BindingType::SampledTexture {
            sample_type,
            dimension: TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

pub(crate) const fn sampler_entry(visibility: ShaderStages) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding: 2,
        visibility,
        ty: BindingType::Sampler(SamplerBindingType::Filtering),
        count: None,
    }
}

pub(crate) const fn lut_output_entry() -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding: 4,
        visibility: ShaderStages::COMPUTE,
        ty: BindingType::StorageTexture {
            access: StorageAccess::WriteOnly,
            format: LUT_FORMAT,
            dimension: TextureViewDimension::D2,
        },
        count: None,
    }
}

// Uniforms are written while the graph is built, so every record attempt (ADR-0022) starts the ring again.
#[derive(Debug)]
pub(crate) struct PerRecordUniforms {
    ring: UniformRing,
    epoch: Option<u64>,
}

impl PerRecordUniforms {
    pub(crate) fn new(device: &dyn DeviceApi, label: &'static str) -> Result<Self, RhiError> {
        Ok(Self {
            ring: UniformRing::new(device, label, 16 << 10)?,
            epoch: None,
        })
    }

    pub(crate) fn write(
        &mut self,
        device: &dyn DeviceApi,
        record_epoch: u64,
        bytes: &[u8],
    ) -> Result<UniformSlice, RhiError> {
        if self.epoch != Some(record_epoch) {
            self.ring.begin_frame();
            self.epoch = Some(record_epoch);
        }
        self.ring.write(device, bytes)
    }
}

#[cfg(test)]
mod tests {
    use naga::valid::{Capabilities, ValidationFlags, Validator};

    use super::*;

    fn assert_valid(label: &str, wgsl: &str) {
        let module = naga::front::wgsl::parse_str(wgsl)
            .unwrap_or_else(|error| panic!("{label}: {}", error.emit_to_string(wgsl)));
        Validator::new(ValidationFlags::all(), Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|error| panic!("{label}: {error:?}"));
    }

    #[test]
    fn every_shader_module_is_valid_wgsl() {
        assert_valid("transmittance", &transmittance_wgsl());
        assert_valid("multiscatter", &multiscatter_wgsl());
        assert_valid("sky view", &sky_view_wgsl());
        assert_valid("sky with depth", &sky_wgsl(true));
        assert_valid("sky without depth", &sky_wgsl(false));
    }
}
