// Image decoding (PNG, JPEG) lives here and only here: the engine crates take raw texels (pathon-texture).

use std::io::Cursor;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::thread;

use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use super::{CpuImage, LoadError};

const RGBA_CHANNELS: usize = 4;

fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G'])
}

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0xd8])
}

fn expand_to_rgba(info: &png::OutputInfo, pixels: &[u8]) -> Result<Vec<u8>, String> {
    let texel_count = info.width as usize * info.height as usize;
    let mut rgba = Vec::with_capacity(texel_count * RGBA_CHANNELS);
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(&pixels[..texel_count * 4]),
        png::ColorType::Rgb => {
            for texel in pixels[..texel_count * 3].as_chunks::<3>().0 {
                rgba.extend_from_slice(&[texel[0], texel[1], texel[2], u8::MAX]);
            }
        }
        png::ColorType::Grayscale => {
            for &gray in &pixels[..texel_count] {
                rgba.extend_from_slice(&[gray, gray, gray, u8::MAX]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for texel in pixels[..texel_count * 2].as_chunks::<2>().0 {
                rgba.extend_from_slice(&[texel[0], texel[0], texel[0], texel[1]]);
            }
        }
        png::ColorType::Indexed => return Err("indexed PNG was not expanded".into()),
    }
    Ok(rgba)
}

fn decode_png(bytes: &[u8]) -> Result<CpuImage, String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let mut pixels = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let info = reader.next_frame(&mut pixels).map_err(|error| error.to_string())?;
    Ok(CpuImage {
        width: info.width,
        height: info.height,
        rgba: expand_to_rgba(&info, &pixels)?,
    })
}

fn decode_jpeg(bytes: &[u8]) -> Result<CpuImage, String> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let rgba = decoder.decode().map_err(|error| format!("{error:?}"))?;
    let info = decoder.info().ok_or("JPEG without a header")?;
    Ok(CpuImage {
        width: u32::from(info.width),
        height: u32::from(info.height),
        rgba,
    })
}

pub(super) fn decode_image(bytes: &[u8]) -> Result<CpuImage, String> {
    if is_png(bytes) {
        decode_png(bytes)
    } else if is_jpeg(bytes) {
        decode_jpeg(bytes)
    } else {
        Err("neither PNG nor JPEG".into())
    }
}

fn collect_results(results: Vec<Option<Result<CpuImage, String>>>) -> Result<Vec<CpuImage>, LoadError> {
    results
        .into_iter()
        .enumerate()
        .map(|(index, result)| match result {
            Some(Ok(image)) => Ok(image),
            Some(Err(message)) => Err(LoadError::Image { index, message }),
            None => Err(LoadError::Image {
                index,
                message: "not decoded".into(),
            }),
        })
        .collect()
}

// The 2K images of a real scene take seconds one after another: workers pull the next undecoded image until none is
// left.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn decode_all(sources: &[Vec<u8>]) -> Result<Vec<CpuImage>, LoadError> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Result<CpuImage, String>>>> = Mutex::new((0..sources.len()).map(|_| None).collect());
    let workers = thread::available_parallelism()
        .map_or(1, usize::from)
        .min(sources.len().max(1));
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(source) = sources.get(index) else {
                        return;
                    };
                    let decoded = decode_image(source);
                    results.lock().expect("a worker only stores a finished result")[index] = Some(decoded);
                }
            });
        }
    });
    collect_results(results.into_inner().expect("the workers are done"))
}

// The browser has no threads here (ADR-0014): one image after the other.
#[cfg(target_arch = "wasm32")]
pub(super) fn decode_all(sources: &[Vec<u8>]) -> Result<Vec<CpuImage>, LoadError> {
    collect_results(sources.iter().map(|source| Some(decode_image(source))).collect())
}

#[cfg(test)]
mod tests {
    use super::decode_image;

    // A 2x1 RGB PNG written by the png crate.
    fn tiny_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("header");
        writer.write_image_data(&[255, 0, 0, 0, 255, 0]).expect("data");
        writer.finish().expect("finish");
        bytes
    }

    #[test]
    fn png_comes_out_as_rgba8() {
        let image = decode_image(&tiny_png()).expect("decodes");
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.rgba, [255, 0, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn unknown_bytes_are_an_error() {
        assert!(decode_image(b"GIF89a").is_err());
    }
}
