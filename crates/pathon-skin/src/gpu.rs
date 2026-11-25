use pathon_api::rhi::{
    BindGroupLayoutDesc, BindGroupLayoutEntry, BindGroupLayoutId, BindingType, BufferDesc, BufferId, BufferUsage,
    ComputePipelineDesc, ComputePipelineId, DeviceApi, RhiError, ShaderModuleDesc, ShaderStage, ShaderStages,
    StorageAccess,
};

pub(crate) const SKIN_WGSL: &str = include_str!("../wgsl/skin.wgsl");
pub(crate) const WORKGROUP_SIZE: u32 = 64;
pub(crate) const FLAG_SKINNED: u32 = 1;
pub(crate) const FLAG_WRITE_PREVIOUS: u32 = 2;

/// Mirror of `Params` in `skin.wgsl`; a test compares the offsets with what naga reflects.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Params {
    pub(crate) vertex_count: u32,
    pub(crate) source_base_vertex: u32,
    pub(crate) position_current: u32,
    pub(crate) position_previous: u32,
    pub(crate) surface_first: u32,
    pub(crate) bone_base: u32,
    pub(crate) bone_count: u32,
    pub(crate) weight_base: u32,
    pub(crate) morph_count: u32,
    pub(crate) morph_vertices: u32,
    pub(crate) flags: u32,
    pub(crate) row_threads: u32,
}

const _: () = assert!(size_of::<Params>() == 48);

fn entry(binding: u32, ty: BindingType) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::COMPUTE,
        ty,
        count: None,
    }
}

fn storage(binding: u32, access: StorageAccess) -> BindGroupLayoutEntry {
    entry(binding, BindingType::StorageBuffer { access })
}

/// The bindings of `skin.wgsl`; a test compares them with the shader's reflection.
pub(crate) fn layout_entries() -> Vec<BindGroupLayoutEntry> {
    vec![
        entry(0, BindingType::UniformBuffer),
        storage(1, StorageAccess::ReadOnly),
        storage(2, StorageAccess::ReadOnly),
        storage(3, StorageAccess::ReadOnly),
        storage(4, StorageAccess::ReadOnly),
        storage(5, StorageAccess::ReadOnly),
        storage(6, StorageAccess::ReadWrite),
        storage(7, StorageAccess::ReadWrite),
    ]
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SkinGpu {
    pub(crate) layout: BindGroupLayoutId,
    pub(crate) pipeline: ComputePipelineId,
    /// Bound where an instance has no skinning stream or no morphs; never read, but a binding must exist.
    pub(crate) placeholder: BufferId,
}

impl SkinGpu {
    pub(crate) fn create(device: &dyn DeviceApi) -> Result<Self, RhiError> {
        let module = device.create_shader_module(&ShaderModuleDesc {
            label: "skin",
            wgsl: SKIN_WGSL,
        })?;
        let layout = device.create_bind_group_layout(&BindGroupLayoutDesc {
            label: "skin",
            entries: layout_entries(),
        })?;
        let pipeline = device.create_compute_pipeline(&ComputePipelineDesc {
            label: "skin",
            bind_group_layouts: vec![layout],
            shader: ShaderStage {
                module,
                entry_point: "main",
                constants: vec![],
            },
            immediate_size: 0,
        })?;
        let placeholder = device.create_buffer(&BufferDesc {
            label: "skin placeholder",
            size: 16,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        })?;
        device.write_buffer(placeholder, 0, &[0; 16])?;
        Ok(Self {
            layout,
            pipeline,
            placeholder,
        })
    }
}

/// A buffer rewritten every frame that doubles when the data outgrows it; the old one is handed back to be released
/// once the frames in flight are done with it.
#[derive(Debug)]
pub(crate) struct GrowBuffer {
    label: &'static str,
    usage: BufferUsage,
    current: Option<(BufferId, u64)>,
}

const MIN_CAPACITY: u64 = 256;

impl GrowBuffer {
    pub(crate) fn new(label: &'static str, usage: BufferUsage) -> Self {
        Self {
            label,
            usage: usage | BufferUsage::COPY_DST,
            current: None,
        }
    }

    pub(crate) fn write(
        &mut self,
        device: &dyn DeviceApi,
        bytes: &[u8],
        retired: &mut Vec<BufferId>,
    ) -> Result<BufferId, RhiError> {
        let needed = (bytes.len() as u64).max(MIN_CAPACITY);
        let buffer = match self.current {
            Some((buffer, capacity)) if capacity >= needed => buffer,
            replaced => {
                retired.extend(replaced.map(|(buffer, _)| buffer));
                let capacity = needed.next_power_of_two();
                let buffer = device.create_buffer(&BufferDesc {
                    label: self.label,
                    size: capacity,
                    usage: self.usage,
                })?;
                self.current = Some((buffer, capacity));
                buffer
            }
        };
        device.write_buffer(buffer, 0, bytes)?;
        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use pathon_shader::{reflect_bind_groups, reflect_buffer_layout};

    use super::*;

    #[test]
    fn the_shader_matches_its_layout() {
        let reflected = reflect_bind_groups(SKIN_WGSL, &["main"]).expect("skin.wgsl");
        assert_eq!(reflected.len(), 1);
        let mut actual = reflected[0].entries.clone();
        actual.sort_by_key(|entry| entry.binding);
        assert_eq!(actual, layout_entries());
    }

    #[test]
    fn params_match_the_shader_struct() {
        let layout = reflect_buffer_layout(SKIN_WGSL, 0, 0).expect("params");
        assert_eq!(layout.struct_size, u32::try_from(size_of::<Params>()).ok());
        let names: Vec<_> = layout.members.iter().map(|member| member.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "vertex_count",
                "source_base_vertex",
                "position_current",
                "position_previous",
                "surface_first",
                "bone_base",
                "bone_count",
                "weight_base",
                "morph_count",
                "morph_vertices",
                "flags",
                "row_threads"
            ]
        );
        for (index, member) in layout.members.iter().enumerate() {
            assert_eq!(member.offset as usize, index * 4, "{}", member.name);
        }
    }
}
