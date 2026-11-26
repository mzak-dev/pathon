// The measuring mode of the viewer (docs/porownanie-obrazu.md): one frame whose `hdr` slot is written as linear
// radiance to a NumPy `.npy` file, with no tone curve, so `tools/scenes/compare.py` can compare it with Cycles.

use std::path::Path;

use pathon::rhi::{
    Access, BufferCopy, BufferDesc, BufferUsage, CommandList, DeviceApi, Extent3d, Format, SubresourceRange,
    TexelLayout, TextureCopy,
};

use super::npy;
use super::screenshot::ScreenshotRequest;
use super::session::Session;
use super::stack::ViewerConfig;
use crate::gltf_loader::LoadedScene;

const LINEAR_FORMAT: Format = Format::Rgba16Float;
const BYTES_PER_PIXEL: u32 = 8;
// Buffer rows of a texture copy must start on 256-byte boundaries (wgpu, D3D12).
const COPY_BYTES_PER_ROW_ALIGNMENT: u32 = 256;

/// Linear RGB radiance, `width * height * 3` values, top row first.
#[derive(Clone, Debug, PartialEq)]
pub struct LinearImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<f32>,
}

fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * fraction * 2.0_f32.powi(-24),
        0x1f if fraction == 0.0 => sign * f32::INFINITY,
        0x1f => f32::NAN,
        _ => sign * (1.0 + fraction / 1024.0) * 2.0_f32.powi(exponent - 15),
    }
}

fn read_linear_texture(
    device: &dyn DeviceApi,
    session: &Session,
    target: pathon::TargetId,
) -> Result<LinearImage, String> {
    let texture = session
        .renderer
        .target_texture(target)
        .ok_or("the offscreen target has no texture")?;
    let desc = device.texture_desc(texture).map_err(|error| error.to_string())?;
    let Extent3d { width, height, .. } = desc.size;
    let packed_row = width * BYTES_PER_PIXEL;
    let padded_row = packed_row.next_multiple_of(COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device
        .create_buffer(&BufferDesc {
            label: "linear dump readback",
            size: u64::from(padded_row) * u64::from(height),
            usage: BufferUsage::MAP_READ | BufferUsage::COPY_DST,
        })
        .map_err(|error| error.to_string())?;
    let mut list = CommandList::new();
    list.texture_access(texture, SubresourceRange::ALL, Access::SAMPLED, Access::COPY_SRC);
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
    list.texture_access(texture, SubresourceRange::ALL, Access::COPY_SRC, Access::SAMPLED);
    let bytes = device.submit(list).and_then(|()| device.read_buffer(readback));
    device.destroy(readback.into());

    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for row in bytes
        .map_err(|error| error.to_string())?
        .chunks_exact(padded_row as usize)
    {
        let (texels, _) = row[..packed_row as usize].as_chunks::<{ BYTES_PER_PIXEL as usize }>();
        for texel in texels {
            for channel in [[texel[0], texel[1]], [texel[2], texel[3]], [texel[4], texel[5]]] {
                rgb.push(half_to_f32(u16::from_le_bytes(channel)));
            }
        }
    }
    Ok(LinearImage { width, height, rgb })
}

/// Averages `factor` x `factor` blocks: the box filter of one output pixel, which is the pixel filter of the Cycles
/// reference.
pub fn box_downsample(image: &LinearImage, factor: u32) -> LinearImage {
    if factor <= 1 {
        return image.clone();
    }
    let (width, height) = (image.width / factor, image.height / factor);
    let weight = 1.0 / (factor * factor) as f32;
    let mut rgb = vec![0.0_f32; width as usize * height as usize * 3];
    for y in 0..height * factor {
        for x in 0..width * factor {
            let source = (y * image.width + x) as usize * 3;
            let target = ((y / factor) * width + x / factor) as usize * 3;
            for channel in 0..3 {
                rgb[target + channel] += image.rgb[source + channel] * weight;
            }
        }
    }
    LinearImage { width, height, rgb }
}

/// Renders `scene` once with the `Linear` tone curve into a half-float target and returns the exposed `hdr` slot.
/// `config.tonemapper` must be `Tonemapper::Linear` and `request` should have the bloom off.
pub fn render_linear(
    scene: &LoadedScene,
    config: &ViewerConfig,
    request: &ScreenshotRequest,
) -> Result<LinearImage, String> {
    let renderer = config.builder().build().map_err(|error| error.to_string())?;
    log::info!("adapter: {}", renderer.device().caps().adapter.name);
    let mut session = Session::new(renderer, scene, config).map_err(|error| error.to_string())?;
    request.place_camera(&mut session)?;
    session.settings = request.settings();
    let target = session
        .renderer
        .add_offscreen_target(request.render_size(), LINEAR_FORMAT)
        .map_err(|error| error.to_string())?;
    let report = session
        .render_settled(target, request.aspect_ratio())
        .map_err(|error| error.to_string())?;
    for diagnostic in &report.diagnostics {
        log::warn!("{diagnostic}");
    }
    log::info!("{}", session.describe());
    let device = std::sync::Arc::clone(session.renderer.device());
    let picture = read_linear_texture(device.as_ref(), &session, target)?;
    Ok(box_downsample(&picture, request.supersample.max(1)))
}

pub fn write_npy(path: &Path, image: &LinearImage) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = npy::encode_f32(&[image.height as usize, image.width as usize, 3], &image.rgb);
    std::fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_decode_to_their_values() {
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0xc000), -2.0);
        assert_eq!(half_to_f32(0x7bff), 65504.0);
        assert_eq!(half_to_f32(0x0001), 2.0_f32.powi(-24));
        assert!(half_to_f32(0x7e00).is_nan());
        assert_eq!(half_to_f32(0x7c00), f32::INFINITY);
    }

    #[test]
    fn a_box_of_four_pixels_is_their_mean() {
        let image = LinearImage {
            width: 2,
            height: 2,
            rgb: vec![0.0, 0.0, 0.0, 4.0, 8.0, 12.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        };
        let averaged = box_downsample(&image, 2);
        assert_eq!((averaged.width, averaged.height), (1, 1));
        assert_eq!(averaged.rgb, vec![1.0, 2.0, 3.0]);
    }
}
