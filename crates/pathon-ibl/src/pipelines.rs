use pathon_api::rhi::{
    BindGroupLayoutDesc, BindGroupLayoutEntry, BindGroupLayoutId, BindingType, ComputePipelineDesc, ComputePipelineId,
    DeviceApi, Format, Resource, RhiError, SamplerBindingType, SamplerDesc, SamplerId, ShaderModuleDesc,
    ShaderModuleId, ShaderStage, ShaderStages, StorageAccess, TextureSampleType, TextureViewDimension,
};

const COMMON: &str = include_str!("../wgsl/common.wgsl");
const EQUIRECT_TO_CUBE: &str = include_str!("../wgsl/equirect_to_cube.wgsl");
const DOWNSAMPLE_CUBE: &str = include_str!("../wgsl/downsample_cube.wgsl");
const PREFILTER: &str = include_str!("../wgsl/prefilter.wgsl");
const IRRADIANCE: &str = include_str!("../wgsl/irradiance.wgsl");
const BRDF_LUT: &str = include_str!("../wgsl/brdf_lut.wgsl");

pub(crate) const WORKGROUP_SIZE: u32 = 8;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Step {
    module: ShaderModuleId,
    pub(crate) layout: BindGroupLayoutId,
    pub(crate) pipeline: ComputePipelineId,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Pipelines {
    pub(crate) equirect_to_cube: Step,
    pub(crate) downsample_cube: Step,
    pub(crate) prefilter: Step,
    pub(crate) irradiance: Step,
    pub(crate) brdf_lut: Step,
    pub(crate) sampler: SamplerId,
}

fn entry(binding: u32, ty: BindingType) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::COMPUTE,
        ty,
        count: None,
    }
}

fn sampled(binding: u32, dimension: TextureViewDimension) -> BindGroupLayoutEntry {
    entry(
        binding,
        BindingType::SampledTexture {
            sample_type: TextureSampleType::Float,
            dimension,
            multisampled: false,
        },
    )
}

fn storage(binding: u32, dimension: TextureViewDimension) -> BindGroupLayoutEntry {
    entry(
        binding,
        BindingType::StorageTexture {
            access: StorageAccess::WriteOnly,
            format: Format::Rgba16Float,
            dimension,
        },
    )
}

fn full_source(body: &str) -> String {
    format!("{COMMON}\n{body}")
}

fn create_step(
    device: &dyn DeviceApi,
    label: &'static str,
    body: &str,
    entries: Vec<BindGroupLayoutEntry>,
) -> Result<Step, RhiError> {
    let wgsl = full_source(body);
    let module = device.create_shader_module(&ShaderModuleDesc { label, wgsl: &wgsl })?;
    let layout = device.create_bind_group_layout(&BindGroupLayoutDesc { label, entries })?;
    let pipeline = device.create_compute_pipeline(&ComputePipelineDesc {
        label,
        bind_group_layouts: vec![layout],
        shader: ShaderStage {
            module,
            entry_point: "main",
            constants: vec![],
        },
        immediate_size: 0,
    })?;
    Ok(Step {
        module,
        layout,
        pipeline,
    })
}

// Label, shader body and the bindings its bind group layout must have; the test below compares them with what naga
// reflects from the shader.
fn definitions() -> [(&'static str, &'static str, Vec<BindGroupLayoutEntry>); 5] {
    use TextureViewDimension::{Cube, D2, D2Array};
    let uniform = entry(0, BindingType::UniformBuffer);
    [
        (
            "ibl equirect to cube",
            EQUIRECT_TO_CUBE,
            vec![uniform, sampled(1, D2), storage(2, D2Array)],
        ),
        (
            "ibl downsample cube",
            DOWNSAMPLE_CUBE,
            vec![sampled(0, D2Array), storage(1, D2Array)],
        ),
        (
            "ibl prefilter",
            PREFILTER,
            vec![
                uniform,
                sampled(1, Cube),
                entry(2, BindingType::Sampler(SamplerBindingType::Filtering)),
                storage(3, D2Array),
            ],
        ),
        (
            "ibl irradiance",
            IRRADIANCE,
            vec![uniform, sampled(1, D2Array), storage(2, D2Array)],
        ),
        ("ibl brdf lut", BRDF_LUT, vec![uniform, storage(1, D2)]),
    ]
}

impl Pipelines {
    pub(crate) fn create(device: &dyn DeviceApi) -> Result<Self, RhiError> {
        let [equirect_to_cube, downsample_cube, prefilter, irradiance, brdf_lut] =
            definitions().map(|(label, body, entries)| create_step(device, label, body, entries));
        Ok(Self {
            equirect_to_cube: equirect_to_cube?,
            downsample_cube: downsample_cube?,
            prefilter: prefilter?,
            irradiance: irradiance?,
            brdf_lut: brdf_lut?,
            sampler: device.create_sampler(&SamplerDesc::LINEAR_CLAMP)?,
        })
    }

    pub(crate) fn release(self, device: &dyn DeviceApi) {
        device.destroy(Resource::Sampler(self.sampler));
        for step in [
            self.equirect_to_cube,
            self.downsample_cube,
            self.prefilter,
            self.irradiance,
            self.brdf_lut,
        ] {
            device.destroy(Resource::ComputePipeline(step.pipeline));
            device.destroy(Resource::BindGroupLayout(step.layout));
            device.destroy(Resource::ShaderModule(step.module));
        }
    }
}

#[cfg(test)]
mod tests {
    use pathon_shader::reflect_bind_groups;

    use super::*;

    #[test]
    fn every_compute_shader_validates_and_matches_its_layout() {
        for (label, body, entries) in definitions() {
            let wgsl = full_source(body);
            let reflected = reflect_bind_groups(&wgsl, &["main"]).unwrap_or_else(|error| panic!("{label}: {error}"));
            assert_eq!(reflected.len(), 1, "{label}: one bind group");
            let mut actual = reflected[0].entries.clone();
            actual.sort_by_key(|entry| entry.binding);
            assert_eq!(actual, entries, "{label}");
        }
    }
}
