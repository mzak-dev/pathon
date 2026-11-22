use pathon_rhi::{
    Access, BufferCopy, BufferDesc, BufferUsage, CommandList, DeviceApi, Extent3d, Origin3d, SubresourceRange,
    TexelLayout, TextureAspect, TextureCopy, TextureId,
};

const ROW_ALIGNMENT: u32 = 256;

/// One mip of one layer with tightly packed rows; the subresource ends in the state it started in.
pub(crate) fn read_mip(device: &dyn DeviceApi, texture: TextureId, state: Access, level: u32, layer: u32) -> Vec<u8> {
    let desc = device.texture_desc(texture).expect("texture");
    let size = desc.size.mip_level(level);
    let texel_bytes = desc.format.block_size_bytes().expect("copyable format");
    let packed_row = size.width * texel_bytes;
    let padded_row = packed_row.next_multiple_of(ROW_ALIGNMENT);
    let buffer = device
        .create_buffer(&BufferDesc {
            label: "mip readback",
            size: u64::from(padded_row * size.height),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("buffer");
    let subresource = SubresourceRange {
        base_mip: level,
        mip_count: 1,
        base_layer: layer,
        layer_count: 1,
    };
    let mut list = CommandList::new();
    list.texture_access(texture, subresource, state, Access::COPY_SRC);
    list.copy_texture_to_buffer(
        TextureCopy {
            texture,
            mip: level,
            origin: Origin3d { x: 0, y: 0, z: layer },
            aspect: TextureAspect::All,
        },
        BufferCopy {
            buffer,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: padded_row,
                rows_per_image: size.height,
            },
        },
        Extent3d::new_2d(size.width, size.height),
    );
    list.texture_access(texture, subresource, Access::COPY_SRC, state);
    device.submit(list).expect("submit");
    let padded = device.read_buffer(buffer).expect("read");
    padded
        .chunks(padded_row as usize)
        .take(size.height as usize)
        .flat_map(|row| &row[..packed_row as usize])
        .copied()
        .collect()
}
