use pathon_rhi::{
    Access, BufferCopy, BufferDesc, BufferUsage, CommandList, DeviceApi, Extent3d, Format, SubresourceRange,
    TexelLayout, TextureCopy, TextureDimension, TextureId,
};

use crate::error::GoldenError;
use crate::image::GoldenImage;

const BYTES_PER_PIXEL: u32 = 4;
// Buffer rows of a texture copy must start on 256-byte boundaries (wgpu, D3D12).
const COPY_BYTES_PER_ROW_ALIGNMENT: u32 = 256;

/// Copies a 2D, single-layer 8-bit RGBA or BGRA texture to the CPU; the texture ends in the same `state` it started in.
pub fn read_texture<D: DeviceApi + ?Sized>(
    device: &D,
    texture: TextureId,
    state: Access,
) -> Result<GoldenImage, GoldenError> {
    let desc = device.texture_desc(texture)?;
    let swap_red_blue = match desc.format {
        Format::Rgba8Unorm | Format::Rgba8UnormSrgb => false,
        Format::Bgra8Unorm | Format::Bgra8UnormSrgb => true,
        other => return Err(GoldenError::UnsupportedReadback(other)),
    };
    let Extent3d {
        width,
        height,
        depth_or_layers,
    } = desc.size;
    if desc.dimension != TextureDimension::D2 || depth_or_layers != 1 || desc.sample_count != 1 {
        return Err(GoldenError::UnsupportedReadback(desc.format));
    }
    let packed_row = width * BYTES_PER_PIXEL;
    let padded_row = packed_row.next_multiple_of(COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&BufferDesc {
        label: "golden readback",
        size: u64::from(padded_row) * u64::from(height),
        usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
    })?;

    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, state, Access::COPY_SRC);
    list.copy_texture_to_buffer(
        TextureCopy::whole(texture),
        BufferCopy {
            buffer: readback,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: padded_row,
                rows_per_image: height,
            },
        },
        Extent3d::new_2d(width, height),
    );
    list.texture_access(texture, SubresourceRange::ALL, Access::COPY_SRC, state);
    let bytes = device.submit(list).and_then(|()| device.read_buffer(readback));
    device.destroy(readback.into());

    let mut rgba = Vec::with_capacity((packed_row * height) as usize);
    for row in bytes?.chunks_exact(padded_row as usize) {
        rgba.extend_from_slice(&row[..packed_row as usize]);
    }
    if swap_red_blue {
        for texel in rgba.as_chunks_mut::<4>().0 {
            texel.swap(0, 2);
        }
    }
    GoldenImage::from_rgba8(width, height, rgba)
}
