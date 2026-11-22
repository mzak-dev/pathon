use naga::{AddressSpace, TypeInner};

use crate::{ShaderError, issues};

/// One field of a struct held by a buffer binding, as the shader lays it out.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MemberLayout {
    pub name: String,
    pub offset: u32,
    pub size: u32,
}

/// How the shader lays out the struct behind a `uniform` or `storage` binding.
///
/// `members` is empty when the binding is not a struct (for example `array<vec4<f32>>`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BufferLayout {
    /// `Some(stride)` when the binding is an array; the struct is then the array element.
    pub element_stride: Option<u32>,
    /// Size of the struct, trailing padding included.
    pub struct_size: Option<u32>,
    pub members: Vec<MemberLayout>,
}

/// Layout of the buffer bound at `@group(group) @binding(binding)`, so a CPU struct can be checked against the shader.
pub fn reflect_buffer_layout(wgsl: &str, group: u32, binding: u32) -> Result<BufferLayout, ShaderError> {
    let (module, _) = issues::parse_and_validate(wgsl).map_err(|issue| crate::reflect::invalid_wgsl(&issue, wgsl))?;

    let variable = module
        .global_variables
        .iter()
        .map(|(_, variable)| variable)
        .find(|variable| {
            variable
                .binding
                .is_some_and(|slot| slot.group == group && slot.binding == binding)
        })
        .ok_or_else(|| ShaderError::UnsupportedBinding {
            group,
            binding,
            reason: "no resource is declared at this binding".to_owned(),
        })?;
    if !matches!(variable.space, AddressSpace::Uniform | AddressSpace::Storage { .. }) {
        return Err(ShaderError::UnsupportedBinding {
            group,
            binding,
            reason: "only uniform and storage buffers have a layout".to_owned(),
        });
    }

    let (element, element_stride) = match &module.types[variable.ty].inner {
        TypeInner::Array { base, stride, .. } => (*base, Some(*stride)),
        _ => (variable.ty, None),
    };
    let TypeInner::Struct { members, span } = &module.types[element].inner else {
        return Ok(BufferLayout {
            element_stride,
            struct_size: None,
            members: Vec::new(),
        });
    };
    let members = members
        .iter()
        .map(|member| MemberLayout {
            name: member.name.clone().unwrap_or_default(),
            offset: member.offset,
            size: module.types[member.ty].inner.size(module.to_ctx()),
        })
        .collect();
    Ok(BufferLayout {
        element_stride,
        struct_size: Some(*span),
        members,
    })
}
