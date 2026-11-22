use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::Path;

use crate::error::GoldenError;
use crate::image::GoldenImage;

pub(crate) fn write_png(path: &Path, image: &GoldenImage) -> Result<(), GoldenError> {
    let failed = |message: String| GoldenError::File {
        path: path.to_path_buf(),
        message,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| failed(error.to_string()))?;
    }
    let file = File::create(path).map_err(|error| failed(error.to_string()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), image.width(), image.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| failed(error.to_string()))?;
    writer
        .write_image_data(image.as_rgba8())
        .and_then(|()| writer.finish())
        .map_err(|error| failed(error.to_string()))
}

pub(crate) fn read_png(path: &Path) -> Result<GoldenImage, GoldenError> {
    let failed = |message: String| GoldenError::File {
        path: path.to_path_buf(),
        message,
    };
    let file = File::open(path).map_err(|error| failed(error.to_string()))?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|error| failed(error.to_string()))?;
    let mut buffer = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or_else(|| failed("image too large".into()))?
    ];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| failed(error.to_string()))?;
    buffer.truncate(info.buffer_size());
    if info.bit_depth != png::BitDepth::Eight {
        return Err(failed(format!("expected an 8-bit PNG, found {:?}", info.bit_depth)));
    }
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[red, green, blue]| [red, green, blue, u8::MAX])
            .collect(),
        other => return Err(failed(format!("expected an RGB or RGBA PNG, found {other:?}"))),
    };
    GoldenImage::from_rgba8(info.width, info.height, rgba)
}
