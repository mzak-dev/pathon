// M4-13: a texture goes through `TextureLoader`: the loader picks the encoding the GPU samples (BCn on desktop, ASTC or
// ETC2 on mobile, RGBA8 where nothing compressed is available) and generates the mip chain in compute. A black and
// white checkerboard shows why mips of an sRGB texture are averaged in linear light: every mip is 188, not 128.

use pathon::rhi::{
    Access, BufferCopy, BufferDesc, BufferUsage, CommandList, Device, DeviceApi, DeviceRequest, Extent3d, Features,
    Origin3d, SubresourceRange, TexelLayout, TextureAspect, TextureCopy, TextureId, TextureUsage,
};
use pathon_texture::{ColorSpace, TextureLoader, TextureSource, Variant};

const SIZE: u32 = 64;
const ROW_ALIGNMENT: u32 = 256;

fn checkerboard() -> Vec<u8> {
    (0..SIZE * SIZE)
        .flat_map(|index| {
            let white = (index % SIZE + index / SIZE).is_multiple_of(2);
            let value = if white { 255 } else { 0 };
            [value, value, value, 255]
        })
        .collect()
}

// The first texel of a mip, as the shader stores it (sRGB-encoded for this texture).
fn first_texel(device: &Device, texture: TextureId, level: u32) -> [u8; 4] {
    let buffer = device
        .create_buffer(&BufferDesc {
            label: "texel readback",
            size: u64::from(ROW_ALIGNMENT),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .expect("readback buffer");
    let mip = SubresourceRange::mip(level);
    let mut list = CommandList::new();
    list.texture_access(texture, mip, Access::SAMPLED, Access::COPY_SRC);
    list.copy_texture_to_buffer(
        TextureCopy {
            texture,
            mip: level,
            origin: Origin3d::default(),
            aspect: TextureAspect::All,
        },
        BufferCopy {
            buffer,
            layout: TexelLayout {
                offset: 0,
                bytes_per_row: ROW_ALIGNMENT,
                rows_per_image: 1,
            },
        },
        Extent3d::new_2d(1, 1),
    );
    list.texture_access(texture, mip, Access::COPY_SRC, Access::SAMPLED);
    device.submit(list).expect("submit");
    let bytes = device.read_buffer(buffer).expect("read");
    [bytes[0], bytes[1], bytes[2], bytes[3]]
}

fn main() {
    pathon_examples::init_logging();
    let device = Device::build(&DeviceRequest {
        label: "texture load",
        optional_features: Features::TEXTURE_COMPRESSION_BC
            | Features::TEXTURE_COMPRESSION_ASTC
            | Features::TEXTURE_COMPRESSION_ETC2,
        ..Default::default()
    })
    .expect("a GPU adapter");

    let pixels = checkerboard();
    let source = TextureSource::new("checkerboard", SIZE, SIZE, ColorSpace::Srgb)
        .with_extra_usage(TextureUsage::COPY_SRC)
        .with_variant(Variant::rgba8(vec![&pixels]));

    let choice = TextureLoader::choose_format(device.caps(), &source)
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
    log::info!(
        "chosen: stored as {:?}, sampled as {:?}{}",
        choice.storage,
        choice.sampling,
        if choice.fallback { " (fallback)" } else { "" }
    );

    let mut loader = TextureLoader::new();
    let texture = loader
        .load(&device, &source)
        .unwrap_or_else(|error| panic!("{}", error.to_diagnostic()));
    log::info!("{} mips written by compute", texture.desc.mip_level_count);
    for level in 0..texture.desc.mip_level_count {
        log::info!("mip {level}: first texel {:?}", first_texel(&device, texture.id, level));
    }
    loader.release(&device);
}
