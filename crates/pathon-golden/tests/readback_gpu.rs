#![cfg(feature = "gpu-tests")]

use pathon_golden::{gpu_or_skip, read_texture};
use pathon_rhi::{
    Access, Device, DeviceApi, DeviceRequest, Extent3d, Format, RhiError, TexelLayout, TextureCopy, TextureDesc,
    TextureUsage,
};

fn texels(width: u32, height: u32) -> Vec<u8> {
    (0..width * height * 4)
        .map(|index| u8::try_from(index % 251).expect("below 256"))
        .collect()
}

fn read_back(format: Format) -> Option<(Vec<u8>, Vec<u8>)> {
    let device = gpu_or_skip("readback_gpu", Device::build(&DeviceRequest::default()), |error| {
        matches!(error, RhiError::NoAdapter(_))
    })?;
    // 10 texels = 40 bytes per row: the copy needs padded rows and the readback has to remove them.
    let (width, height) = (10, 3);
    let texture = device
        .create_texture(&TextureDesc::new_2d(
            "readback source",
            width,
            height,
            format,
            TextureUsage::COPY_SRC | TextureUsage::COPY_DST,
        ))
        .expect("texture");
    let written = texels(width, height);
    device
        .write_texture(
            &TextureCopy::whole(texture),
            &written,
            TexelLayout {
                offset: 0,
                bytes_per_row: width * 4,
                rows_per_image: height,
            },
            Extent3d::new_2d(width, height),
        )
        .expect("write");
    let image = read_texture(&device, texture, Access::COPY_DST).expect("read back");
    assert_eq!((image.width(), image.height()), (width, height));
    Some((written, image.as_rgba8().to_vec()))
}

#[test]
fn rgba_rows_come_back_without_the_copy_padding() {
    let Some((written, read)) = read_back(Format::Rgba8Unorm) else {
        return;
    };
    assert_eq!(read, written);
}

#[test]
fn bgra_texels_come_back_as_rgba() {
    let Some((written, read)) = read_back(Format::Bgra8Unorm) else {
        return;
    };
    let swapped: Vec<u8> = written
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[blue, green, red, alpha]| [red, green, blue, alpha])
        .collect();
    assert_eq!(read, swapped);
}
