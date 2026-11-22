use std::collections::BTreeMap;
use std::num::NonZeroU32;

use naga::valid::{GlobalUse, ModuleInfo};
use naga::{AddressSpace, ArraySize, Handle, ImageClass, ImageDimension, Module, ScalarKind, TypeInner};
use pathon_rhi_types::{
    BindGroupLayoutDesc, BindGroupLayoutEntry, BindingType, Format, SamplerBindingType, ShaderStages, StorageAccess,
    TextureSampleType, TextureViewDimension,
};

use crate::{ShaderError, issues};

const REFLECTED_LABEL: &str = "reflected";
// Bounds the layout list; far above the groups a pipeline layout can hold, far below an overflowing index.
const MAX_BIND_GROUPS: u32 = 32;

type SlotKey = (u32, u32);

#[derive(Clone, Copy, PartialEq, Eq)]
struct ResourceKind {
    ty: BindingType,
    count: Option<NonZeroU32>,
}

struct SlotUse {
    kind: ResourceKind,
    visibility: ShaderStages,
}

/// Bind group layouts of the resources the given entry points use, indexed by group number.
pub fn reflect_bind_groups(wgsl: &str, entry_points: &[&str]) -> Result<Vec<BindGroupLayoutDesc>, ShaderError> {
    let (module, info) = issues::parse_and_validate(wgsl).map_err(|issue| invalid_wgsl(&issue, wgsl))?;
    let selected = select_entry_points(&module, entry_points)?;

    let mut slots = BTreeMap::new();
    for (handle, variable) in module.global_variables.iter() {
        let Some(resource_binding) = variable.binding else {
            continue;
        };
        let visibility = visibility_of_users(&info, &selected, handle);
        if visibility.is_empty() {
            continue;
        }
        let key = (resource_binding.group, resource_binding.binding);
        if key.0 >= MAX_BIND_GROUPS {
            return Err(ShaderError::UnsupportedBinding {
                group: key.0,
                binding: key.1,
                reason: format!("group index must be below {MAX_BIND_GROUPS}"),
            });
        }
        let kind =
            resource_kind(&module, variable.space, variable.ty).map_err(|reason| ShaderError::UnsupportedBinding {
                group: key.0,
                binding: key.1,
                reason,
            })?;
        merge_into_slots(&mut slots, key, kind, visibility)?;
    }
    Ok(group_layouts(slots))
}

fn invalid_wgsl(issue: &issues::NagaIssue, wgsl: &str) -> ShaderError {
    let diagnostic = issues::naga_diagnostic(issue, wgsl, None);
    ShaderError::InvalidWgsl {
        message: diagnostic.message.clone(),
        diagnostic: Box::new(diagnostic),
    }
}

struct SelectedEntryPoint {
    index: usize,
    stage: ShaderStages,
}

fn select_entry_points(module: &Module, names: &[&str]) -> Result<Vec<SelectedEntryPoint>, ShaderError> {
    let mut selected = Vec::new();
    for name in names {
        let before = selected.len();
        for (index, entry_point) in module.entry_points.iter().enumerate() {
            if entry_point.name != *name {
                continue;
            }
            let stage = match entry_point.stage {
                naga::ShaderStage::Vertex => ShaderStages::VERTEX,
                naga::ShaderStage::Fragment => ShaderStages::FRAGMENT,
                naga::ShaderStage::Compute => ShaderStages::COMPUTE,
                _ => {
                    return Err(ShaderError::UnsupportedStage {
                        entry_point: entry_point.name.clone(),
                    });
                }
            };
            selected.push(SelectedEntryPoint { index, stage });
        }
        if selected.len() == before {
            return Err(ShaderError::UnknownEntryPoint {
                name: (*name).to_owned(),
            });
        }
    }
    Ok(selected)
}

fn visibility_of_users(
    info: &ModuleInfo,
    selected: &[SelectedEntryPoint],
    variable: Handle<naga::GlobalVariable>,
) -> ShaderStages {
    selected
        .iter()
        .filter(|entry_point| info.get_entry_point(entry_point.index)[variable] != GlobalUse::empty())
        .fold(ShaderStages::empty(), |visibility, entry_point| {
            visibility | entry_point.stage
        })
}

fn merge_into_slots(
    slots: &mut BTreeMap<SlotKey, SlotUse>,
    key: SlotKey,
    kind: ResourceKind,
    visibility: ShaderStages,
) -> Result<(), ShaderError> {
    match slots.get_mut(&key) {
        None => {
            slots.insert(key, SlotUse { kind, visibility });
        }
        Some(existing) if existing.kind == kind => existing.visibility |= visibility,
        Some(_) => {
            return Err(ShaderError::BindingConflict {
                group: key.0,
                binding: key.1,
            });
        }
    }
    Ok(())
}

fn group_layouts(slots: BTreeMap<SlotKey, SlotUse>) -> Vec<BindGroupLayoutDesc> {
    let group_count = slots.keys().map(|(group, _)| group + 1).max().unwrap_or(0);
    let mut layouts: Vec<BindGroupLayoutDesc> = (0..group_count)
        .map(|_| BindGroupLayoutDesc {
            label: REFLECTED_LABEL,
            entries: Vec::new(),
        })
        .collect();
    for ((group, binding), used) in slots {
        layouts[group as usize].entries.push(BindGroupLayoutEntry {
            binding,
            visibility: used.visibility,
            ty: used.kind.ty,
            count: used.kind.count,
        });
    }
    layouts
}

fn resource_kind(module: &Module, space: AddressSpace, ty: Handle<naga::Type>) -> Result<ResourceKind, String> {
    let (element, count) = match &module.types[ty].inner {
        TypeInner::BindingArray { base, size } => (*base, Some(constant_count(*size)?)),
        _ => (ty, None),
    };
    let binding_type = match space {
        AddressSpace::Uniform => BindingType::UniformBuffer,
        AddressSpace::Storage { access } => BindingType::StorageBuffer {
            access: if access.contains(naga::StorageAccess::STORE) {
                StorageAccess::ReadWrite
            } else {
                StorageAccess::ReadOnly
            },
        },
        AddressSpace::Handle => handle_binding_type(&module.types[element].inner)?,
        other => return Err(format!("address space {other:?} cannot hold a binding")),
    };
    Ok(ResourceKind {
        ty: binding_type,
        count,
    })
}

fn constant_count(size: ArraySize) -> Result<NonZeroU32, String> {
    match size {
        ArraySize::Constant(count) => Ok(count),
        ArraySize::Pending(_) => Err("a binding array sized by an override has no fixed count".to_owned()),
        ArraySize::Dynamic => Err("a runtime-sized binding array has no fixed count".to_owned()),
    }
}

fn handle_binding_type(inner: &TypeInner) -> Result<BindingType, String> {
    match inner {
        TypeInner::Sampler { comparison: false } => Ok(BindingType::Sampler(SamplerBindingType::Filtering)),
        TypeInner::Sampler { comparison: true } => Ok(BindingType::Sampler(SamplerBindingType::Comparison)),
        TypeInner::Image { dim, arrayed, class } => image_binding_type(*dim, *arrayed, *class),
        TypeInner::AccelerationStructure { .. } => Err("acceleration structures have no RHI equivalent".to_owned()),
        other => Err(format!("type {other:?} has no RHI binding equivalent")),
    }
}

fn image_binding_type(dim: ImageDimension, arrayed: bool, class: ImageClass) -> Result<BindingType, String> {
    let dimension = texture_view_dimension(dim, arrayed)?;
    match class {
        ImageClass::Sampled { kind, multi } => Ok(BindingType::SampledTexture {
            sample_type: sample_type(kind, multi)?,
            dimension,
            multisampled: multi,
        }),
        ImageClass::Depth { multi } => Ok(BindingType::SampledTexture {
            sample_type: TextureSampleType::Depth,
            dimension,
            multisampled: multi,
        }),
        ImageClass::Storage { format, access } => Ok(BindingType::StorageTexture {
            access: storage_texture_access(access)?,
            format: storage_format(format)?,
            dimension,
        }),
        ImageClass::External => Err("external textures have no RHI equivalent".to_owned()),
    }
}

fn texture_view_dimension(dim: ImageDimension, arrayed: bool) -> Result<TextureViewDimension, String> {
    match (dim, arrayed) {
        (ImageDimension::D1, false) => Ok(TextureViewDimension::D1),
        (ImageDimension::D2, false) => Ok(TextureViewDimension::D2),
        (ImageDimension::D2, true) => Ok(TextureViewDimension::D2Array),
        (ImageDimension::D3, false) => Ok(TextureViewDimension::D3),
        (ImageDimension::Cube, false) => Ok(TextureViewDimension::Cube),
        (ImageDimension::Cube, true) => Ok(TextureViewDimension::CubeArray),
        (dim, true) => Err(format!("arrayed {dim:?} textures have no RHI equivalent")),
    }
}

fn sample_type(kind: ScalarKind, multisampled: bool) -> Result<TextureSampleType, String> {
    match kind {
        // wgpu rejects filterable multisampled textures, and they cannot be sampled anyway.
        ScalarKind::Float if multisampled => Ok(TextureSampleType::UnfilterableFloat),
        ScalarKind::Float => Ok(TextureSampleType::Float),
        ScalarKind::Sint => Ok(TextureSampleType::Sint),
        ScalarKind::Uint => Ok(TextureSampleType::Uint),
        other => Err(format!("textures sampled as {other:?} have no RHI equivalent")),
    }
}

fn storage_texture_access(access: naga::StorageAccess) -> Result<StorageAccess, String> {
    if access.contains(naga::StorageAccess::ATOMIC) {
        return Err("atomic storage textures have no RHI equivalent".to_owned());
    }
    let loads = access.contains(naga::StorageAccess::LOAD);
    let stores = access.contains(naga::StorageAccess::STORE);
    match (loads, stores) {
        (true, true) => Ok(StorageAccess::ReadWrite),
        (true, false) => Ok(StorageAccess::ReadOnly),
        (false, true) => Ok(StorageAccess::WriteOnly),
        (false, false) => Err("a storage texture with no access has no RHI equivalent".to_owned()),
    }
}

fn storage_format(format: naga::StorageFormat) -> Result<Format, String> {
    use naga::StorageFormat as Naga;
    match format {
        Naga::R8Unorm => Ok(Format::R8Unorm),
        Naga::R8Uint => Ok(Format::R8Uint),
        Naga::Rg8Unorm => Ok(Format::Rg8Unorm),
        Naga::Rgba8Unorm => Ok(Format::Rgba8Unorm),
        Naga::Bgra8Unorm => Ok(Format::Bgra8Unorm),
        Naga::R16Float => Ok(Format::R16Float),
        Naga::R16Unorm => Ok(Format::R16Unorm),
        Naga::Rg16Float => Ok(Format::Rg16Float),
        Naga::Rgba16Float => Ok(Format::Rgba16Float),
        Naga::Rgba16Unorm => Ok(Format::Rgba16Unorm),
        Naga::R32Float => Ok(Format::R32Float),
        Naga::R32Uint => Ok(Format::R32Uint),
        Naga::Rg32Float => Ok(Format::Rg32Float),
        Naga::Rg32Uint => Ok(Format::Rg32Uint),
        Naga::Rgba32Float => Ok(Format::Rgba32Float),
        Naga::Rgba32Uint => Ok(Format::Rgba32Uint),
        Naga::Rgb10a2Unorm => Ok(Format::Rgb10a2Unorm),
        Naga::Rg11b10Ufloat => Ok(Format::Rg11b10Ufloat),
        other => Err(format!("storage format {other:?} has no RHI equivalent")),
    }
}
