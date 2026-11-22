use std::collections::HashMap;
use std::num::NonZeroU32;

use pathon_api::rhi::{
    Access, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc, BindGroupLayoutEntry, BindGroupLayoutId,
    BindingResource, BindingType, CommandList, ComputePassDesc, ComputePipelineDesc, ComputePipelineId, DeviceApi,
    Format, FormatCaps, Resource, ShaderModuleDesc, ShaderModuleId, ShaderStage, ShaderStages, StorageAccess,
    SubresourceRange, TextureId, TextureUsage, TextureViewDesc, TextureViewDimension,
};

use crate::error::TextureError;
use crate::source::ColorSpace;

const WORKGROUP_SIZE: u32 = 8;
const SHADER: &str = include_str!("mipmap.wgsl");

/// How texels are averaged into the next mip.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum MipMode {
    /// Plain average of the stored values.
    Linear,
    /// 8-bit colour stored sRGB-encoded: decode, average in linear light, encode.
    Srgb,
    /// Normals stored as `n * 0.5 + 0.5`: average the vectors and renormalize, so mips do not flatten the relief.
    Normal,
}

impl From<ColorSpace> for MipMode {
    fn from(color_space: ColorSpace) -> Self {
        match color_space {
            ColorSpace::Srgb => Self::Srgb,
            ColorSpace::Linear => Self::Linear,
            ColorSpace::Normal => Self::Normal,
        }
    }
}

impl MipMode {
    fn shader_constant(self) -> u32 {
        match self {
            Self::Linear => 0,
            Self::Srgb => 1,
            Self::Normal => 2,
        }
    }
}

/// Formats the mip shader averages: the storage-writable ones the engine supports.
pub fn can_generate_mips(format: Format) -> bool {
    matches!(format, Format::Rgba8Unorm | Format::Rgba16Float)
}

fn storage_format_name(format: Format) -> Option<&'static str> {
    match format {
        Format::Rgba8Unorm => Some("rgba8unorm"),
        Format::Rgba16Float => Some("rgba16float"),
        _ => None,
    }
}

#[derive(Default, Debug)]
struct FormatPipelines {
    module: Option<ShaderModuleId>,
    layout: Option<BindGroupLayoutId>,
}

/// Mip chains written by compute, one dispatch per level.
///
/// Pipelines are created on first use and kept; call [`release`](Self::release) before the device goes away.
#[derive(Default, Debug)]
pub struct MipmapGenerator {
    per_format: HashMap<Format, FormatPipelines>,
    pipelines: HashMap<(Format, MipMode), ComputePipelineId>,
}

impl MipmapGenerator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fills mips `1..` of every layer from mip 0 and leaves the whole texture in `Access::SAMPLED`.
    ///
    /// The texture needs `SAMPLED | STORAGE` usage, a format for which [`can_generate_mips`] holds, and mip 0 written
    /// and in the state `mip0_state` (`COPY_DST` right after `write_texture`). The commands are submitted.
    pub fn generate(
        &mut self,
        device: &dyn DeviceApi,
        texture: TextureId,
        mode: MipMode,
        mip0_state: Access,
    ) -> Result<(), TextureError> {
        let desc = device.texture_desc(texture)?;
        let unsupported = |reason: String| TextureError::MipGeneration {
            label: desc.label,
            reason,
        };
        if !can_generate_mips(desc.format) {
            return Err(unsupported(format!("{:?} is not averaged by compute", desc.format)));
        }
        if !desc.usage.contains(TextureUsage::SAMPLED | TextureUsage::STORAGE) {
            return Err(unsupported("the texture needs SAMPLED and STORAGE usage".into()));
        }
        if !device.caps().format(desc.format).contains(FormatCaps::STORAGE) {
            return Err(unsupported(format!(
                "the device cannot write {:?} from compute",
                desc.format
            )));
        }

        let pipeline = self.pipeline(device, desc.format, mode)?;
        let layout = self.per_format[&desc.format]
            .layout
            .ok_or_else(|| unsupported("no layout".into()))?;
        let mut list = CommandList::new();
        if mip0_state != Access::SAMPLED {
            list.texture_access(texture, SubresourceRange::mip(0), mip0_state, Access::SAMPLED);
        }
        let mut temporaries = Vec::new();
        let result = (|| -> Result<(), TextureError> {
            for level in 1..desc.mip_level_count {
                let target = desc.size.mip_level(level);
                let mut groups = Vec::new();
                for layer in 0..desc.array_layers() {
                    let view = |base_mip| TextureViewDesc {
                        dimension: Some(TextureViewDimension::D2),
                        base_mip,
                        mip_count: Some(1),
                        base_layer: layer,
                        layer_count: Some(1),
                        ..TextureViewDesc::default()
                    };
                    let source = device.create_texture_view(texture, &view(level - 1))?;
                    temporaries.push(Resource::TextureView(source));
                    let destination = device.create_texture_view(texture, &view(level))?;
                    temporaries.push(Resource::TextureView(destination));
                    let group = device.create_bind_group(&BindGroupDesc {
                        label: "mip level",
                        layout,
                        entries: vec![
                            BindGroupEntry {
                                binding: 0,
                                resource: BindingResource::TextureView(source),
                            },
                            BindGroupEntry {
                                binding: 1,
                                resource: BindingResource::TextureView(destination),
                            },
                        ],
                    })?;
                    temporaries.push(Resource::BindGroup(group));
                    groups.push(group);
                }
                list.texture_access(
                    texture,
                    SubresourceRange::mip(level),
                    Access::empty(),
                    Access::STORAGE_WRITE,
                );
                {
                    let mut pass = list.compute_pass(ComputePassDesc {
                        label: "generate mip",
                        timestamps: None,
                    });
                    pass.set_pipeline(pipeline);
                    for group in groups {
                        pass.set_bind_group(0, group);
                        pass.dispatch(
                            target.width.div_ceil(WORKGROUP_SIZE),
                            target.height.div_ceil(WORKGROUP_SIZE),
                            1,
                        );
                    }
                }
                list.texture_access(
                    texture,
                    SubresourceRange::mip(level),
                    Access::STORAGE_WRITE,
                    Access::SAMPLED,
                );
            }
            Ok(device.submit(std::mem::take(&mut list))?)
        })();
        temporaries
            .into_iter()
            .rev()
            .for_each(|resource| device.destroy(resource));
        result
    }

    pub fn release(&mut self, device: &dyn DeviceApi) {
        for (_, pipeline) in self.pipelines.drain() {
            device.destroy(Resource::ComputePipeline(pipeline));
        }
        for (_, objects) in self.per_format.drain() {
            objects
                .layout
                .into_iter()
                .for_each(|layout| device.destroy(Resource::BindGroupLayout(layout)));
            objects
                .module
                .into_iter()
                .for_each(|module| device.destroy(Resource::ShaderModule(module)));
        }
    }

    fn pipeline(
        &mut self,
        device: &dyn DeviceApi,
        format: Format,
        mode: MipMode,
    ) -> Result<ComputePipelineId, TextureError> {
        if let Some(pipeline) = self.pipelines.get(&(format, mode)) {
            return Ok(*pipeline);
        }
        let (module, layout) = self.format_objects(device, format)?;
        let pipeline = device.create_compute_pipeline(&ComputePipelineDesc {
            label: "generate mip",
            bind_group_layouts: vec![layout],
            shader: ShaderStage {
                module,
                entry_point: "main",
                constants: vec![("MODE", mode.shader_constant().into())],
            },
            immediate_size: 0,
        })?;
        self.pipelines.insert((format, mode), pipeline);
        Ok(pipeline)
    }

    fn format_objects(
        &mut self,
        device: &dyn DeviceApi,
        format: Format,
    ) -> Result<(ShaderModuleId, BindGroupLayoutId), TextureError> {
        let objects = self.per_format.entry(format).or_default();
        if let (Some(module), Some(layout)) = (objects.module, objects.layout) {
            return Ok((module, layout));
        }
        let name = storage_format_name(format).ok_or(TextureError::MipGeneration {
            label: "",
            reason: format!("{format:?} has no storage format name"),
        })?;
        let wgsl = SHADER.replace("STORAGE_FORMAT", name);
        let module = device.create_shader_module(&ShaderModuleDesc {
            label: "generate mip",
            wgsl: &wgsl,
        })?;
        objects.module = Some(module);
        let entry = |binding, ty| BindGroupLayoutEntry {
            binding,
            visibility: ShaderStages::COMPUTE,
            ty,
            count: None::<NonZeroU32>,
        };
        let layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "generate mip",
            entries: vec![
                entry(0, BindingType::texture_2d()),
                entry(
                    1,
                    BindingType::StorageTexture {
                        access: StorageAccess::WriteOnly,
                        format,
                        dimension: TextureViewDimension::D2,
                    },
                ),
            ],
        })?;
        objects.layout = Some(layout);
        Ok((module, layout))
    }
}
