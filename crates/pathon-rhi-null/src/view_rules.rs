use pathon_rhi_types::{
    RhiError, SubresourceRange, TextureDesc, TextureDimension, TextureViewDesc, TextureViewDimension,
};

use crate::device::NullTexture;

fn invalid(label: &str, problem: impl std::fmt::Display) -> RhiError {
    RhiError::validation("R0109", format!("texture '{label}': {problem}"))
}

pub(crate) fn check_view_formats(desc: &TextureDesc) -> Result<(), RhiError> {
    for view_format in desc.view_formats {
        if !desc.format.is_view_compatible_with(*view_format) {
            return Err(invalid(
                desc.label,
                format!(
                    "view format {view_format:?} is not compatible with {:?}; only its sRGB or non-sRGB twin is",
                    desc.format
                ),
            ));
        }
    }
    Ok(())
}

// The layers a view covers when `layer_count` is left open; mirrors wgpu, where only array views default to the rest.
fn default_layer_count(dimension: Option<TextureViewDimension>, layers: u32, base_layer: u32) -> u32 {
    match dimension {
        Some(TextureViewDimension::D1 | TextureViewDimension::D2 | TextureViewDimension::D3) => 1,
        Some(TextureViewDimension::Cube) => TextureDesc::CUBE_FACES,
        Some(TextureViewDimension::D2Array | TextureViewDimension::CubeArray) | None => {
            layers.saturating_sub(base_layer)
        }
    }
}

fn check_dimension(desc: &TextureDesc, dimension: TextureViewDimension, layer_count: u32) -> Result<(), RhiError> {
    let (texture_dimension, layers) = (desc.dimension, desc.array_layers());
    let (fits, requirement) = match dimension {
        TextureViewDimension::D1 => (
            texture_dimension == TextureDimension::D1 && layer_count == 1,
            "a 1D texture",
        ),
        TextureViewDimension::D2 => (
            texture_dimension == TextureDimension::D2 && layer_count == 1,
            "a 2D texture and exactly one layer",
        ),
        TextureViewDimension::D2Array => (texture_dimension == TextureDimension::D2, "a 2D texture"),
        TextureViewDimension::Cube => (
            desc.is_cube_compatible() && layer_count == TextureDesc::CUBE_FACES,
            "a single-sampled square 2D texture with at least 6 layers, and exactly 6 layers in the view",
        ),
        TextureViewDimension::CubeArray => (
            desc.is_cube_compatible() && layer_count.is_multiple_of(TextureDesc::CUBE_FACES),
            "a single-sampled square 2D texture and a multiple of 6 layers in the view",
        ),
        TextureViewDimension::D3 => (
            texture_dimension == TextureDimension::D3 && layer_count == 1,
            "a 3D texture",
        ),
    };
    if fits {
        return Ok(());
    }
    Err(invalid(
        desc.label,
        format!(
            "a {dimension:?} view needs {requirement}; the texture is {texture_dimension:?} {}x{} with {layers} layers and the view asks for {layer_count}",
            desc.size.width, desc.size.height
        ),
    ))
}

// Validates a view the way a strict backend would and returns the subresources it covers.
pub(crate) fn checked_view_range(texture: &NullTexture, view: &TextureViewDesc) -> Result<SubresourceRange, RhiError> {
    let desc = &texture.desc;
    let layer_count = view
        .layer_count
        .unwrap_or_else(|| default_layer_count(view.dimension, desc.array_layers(), view.base_layer));
    if let Some(dimension) = view.dimension {
        check_dimension(desc, dimension, layer_count)?;
    }
    let range = SubresourceRange {
        base_mip: view.base_mip,
        mip_count: view.mip_count.unwrap_or(u32::MAX),
        base_layer: view.base_layer,
        layer_count,
    };
    texture.check_range(range)?;
    let range = range.resolve(desc.mip_level_count, desc.array_layers());
    if range.mip_count == 0 || range.layer_count == 0 {
        return Err(invalid(desc.label, "the view selects no subresources"));
    }
    if let Some(format) = view.format
        && format != desc.format
        && !desc.view_formats.contains(&format)
    {
        return Err(invalid(
            desc.label,
            format!("view format {format:?} is not the texture format and is missing from `view_formats`"),
        ));
    }
    Ok(range)
}
