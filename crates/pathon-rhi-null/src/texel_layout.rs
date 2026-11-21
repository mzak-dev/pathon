use pathon_rhi_types::{Extent3d, Format, RhiError, TexelLayout};

// Same value as wgpu's `COPY_BYTES_PER_ROW_ALIGNMENT`; a native backend needs it for buffer copies.
const COPY_BYTES_PER_ROW_ALIGNMENT: u32 = 256;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RowAlignment {
    Required,
    NotRequired,
}

fn invalid_layout(message: String) -> RhiError {
    RhiError::validation("R0113", message)
}

// Formats without a defined memory layout (combined depth-stencil) are not checked here; the copy itself is rejected
// by the backend.
pub(crate) fn check_texel_layout(
    format: Format,
    layout: TexelLayout,
    size: Extent3d,
    alignment: RowAlignment,
) -> Result<(), RhiError> {
    let Some(block_bytes) = format.block_size_bytes() else {
        return Ok(());
    };
    let (block_width, block_height) = format.block_dimensions();
    let row_bytes = u64::from(size.width.div_ceil(block_width)) * u64::from(block_bytes);
    let rows = size.height.div_ceil(block_height);
    if u64::from(layout.bytes_per_row) < row_bytes {
        return Err(invalid_layout(format!(
            "bytes_per_row {} is smaller than a row of {row_bytes} bytes",
            layout.bytes_per_row
        )));
    }
    if alignment == RowAlignment::Required && !layout.bytes_per_row.is_multiple_of(COPY_BYTES_PER_ROW_ALIGNMENT) {
        return Err(invalid_layout(format!(
            "bytes_per_row {} is not a multiple of {COPY_BYTES_PER_ROW_ALIGNMENT}",
            layout.bytes_per_row
        )));
    }
    if size.depth_or_layers > 1 && layout.rows_per_image < rows {
        return Err(invalid_layout(format!(
            "rows_per_image {} is smaller than the {rows} rows of an image",
            layout.rows_per_image
        )));
    }
    Ok(())
}
