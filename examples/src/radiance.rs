// A reader for Radiance `.hdr` pictures (RGBE, flat or run-length encoded scanlines): the format environment maps come
// in. The renderer core does no I/O, so loading a file is the example's job.

use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub struct RadianceError(pub &'static str);

impl fmt::Display for RadianceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Radiance picture: {}", self.0)
    }
}

impl std::error::Error for RadianceError {}

#[derive(Debug, PartialEq)]
pub struct RadianceImage {
    pub width: u32,
    pub height: u32,
    /// Linear RGB radiance, `width * height * 3` values, top row first.
    pub rgb: Vec<f32>,
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], RadianceError> {
        let end = self.position.checked_add(count).filter(|end| *end <= self.bytes.len());
        let end = end.ok_or(RadianceError("the file ends early"))?;
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn line(&mut self) -> Result<&'a str, RadianceError> {
        let rest = &self.bytes[self.position..];
        let length = rest
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or(RadianceError("the header ends early"))?;
        self.position += length + 1;
        std::str::from_utf8(&rest[..length]).map_err(|_| RadianceError("the header is not text"))
    }
}

fn read_resolution(line: &str) -> Result<(u32, u32), RadianceError> {
    let invalid = || RadianceError("unsupported resolution line (only `-Y height +X width` is read)");
    let fields: Vec<&str> = line.split_whitespace().collect();
    let ["-Y", height, "+X", width] = fields[..] else {
        return Err(invalid());
    };
    let height = height.parse().map_err(|_| invalid())?;
    let width = width.parse().map_err(|_| invalid())?;
    Ok((width, height))
}

// One scanline as RGBE quadruples: new-style run-length encoding when the marker matches, plain texels otherwise.
fn read_scanline(cursor: &mut Cursor<'_>, width: usize) -> Result<Vec<[u8; 4]>, RadianceError> {
    let marker = cursor.bytes.get(cursor.position..cursor.position + 4);
    let run_length_encoded = (8..0x8000).contains(&width)
        && marker.is_some_and(|marker| {
            marker[0] == 2 && marker[1] == 2 && usize::from(marker[2]) << 8 | usize::from(marker[3]) == width
        });
    if !run_length_encoded {
        return Ok(cursor.take(width * 4)?.as_chunks::<4>().0.to_vec());
    }
    cursor.take(4)?;
    let mut texels = vec![[0_u8; 4]; width];
    for channel in 0..4 {
        let mut filled = 0;
        while filled < width {
            let count = cursor.take(1)?[0];
            if count > 128 {
                let run = usize::from(count - 128);
                let value = cursor.take(1)?[0];
                if filled + run > width {
                    return Err(RadianceError("a run crosses the end of its scanline"));
                }
                texels[filled..filled + run]
                    .iter_mut()
                    .for_each(|texel| texel[channel] = value);
                filled += run;
            } else {
                let run = usize::from(count);
                if run == 0 || filled + run > width {
                    return Err(RadianceError(
                        "a literal run is empty or crosses the end of its scanline",
                    ));
                }
                for (texel, value) in texels[filled..filled + run].iter_mut().zip(cursor.take(run)?) {
                    texel[channel] = *value;
                }
                filled += run;
            }
        }
    }
    Ok(texels)
}

pub fn read(bytes: &[u8]) -> Result<RadianceImage, RadianceError> {
    let mut cursor = Cursor { bytes, position: 0 };
    if !cursor.line()?.starts_with("#?") {
        return Err(RadianceError("missing the #? signature"));
    }
    loop {
        let line = cursor.line()?;
        if line.is_empty() {
            break;
        }
        if let Some(format) = line.strip_prefix("FORMAT=")
            && format != "32-bit_rle_rgbe"
        {
            return Err(RadianceError("only FORMAT=32-bit_rle_rgbe is supported"));
        }
    }
    let (width, height) = read_resolution(cursor.line()?)?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 1 << 28 {
        return Err(RadianceError("the picture is empty or implausibly large"));
    }
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for _ in 0..height {
        for [r, g, b, exponent] in read_scanline(&mut cursor, width as usize)? {
            let scale = if exponent == 0 {
                0.0
            } else {
                2.0_f32.powi(i32::from(exponent) - 136)
            };
            rgb.extend([r, g, b].map(|mantissa| f32::from(mantissa) * scale));
        }
    }
    Ok(RadianceImage { width, height, rgb })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(width: u32, height: u32) -> Vec<u8> {
        format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {height} +X {width}\n").into_bytes()
    }

    #[test]
    fn flat_scanlines_decode_to_linear_radiance() {
        let mut file = header(2, 1);
        // Exponent 129 scales the mantissas by 2^(129 - 136) = 2^-7: (128, 64, 0) becomes (1, 0.5, 0).
        file.extend([128, 64, 0, 129, 255, 255, 255, 0]);
        let image = read(&file).expect("picture");
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.rgb, [1.0, 0.5, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn run_length_encoded_scanlines_decode() {
        let width = 8_u32;
        let mut file = header(width, 1);
        file.extend([2, 2, 0, 8]);
        // Red: one run of 8; green: a literal of 3 then a run of 5; blue and exponent: a run of 8 each.
        file.extend([128 + 8, 10]);
        file.extend([3, 1, 2, 3, 128 + 5, 9]);
        file.extend([128 + 8, 0]);
        file.extend([128 + 8, 136]);
        let image = read(&file).expect("picture");
        let reds: Vec<f32> = image.rgb.as_chunks::<3>().0.iter().map(|texel| texel[0]).collect();
        let greens: Vec<f32> = image.rgb.as_chunks::<3>().0.iter().map(|texel| texel[1]).collect();
        assert_eq!(reds, [10.0; 8]);
        assert_eq!(greens, [1.0, 2.0, 3.0, 9.0, 9.0, 9.0, 9.0, 9.0]);
    }

    #[test]
    fn broken_files_are_errors_and_not_panics() {
        assert!(read(b"not a picture").is_err());
        assert!(read(&header(4, 4)).is_err(), "no pixel data");
        let mut other_format = b"#?RADIANCE\nFORMAT=32-bit_rle_xyze\n\n-Y 1 +X 1\n".to_vec();
        other_format.extend([0; 4]);
        assert!(read(&other_format).is_err());
        let mut overrun = header(8, 1);
        overrun.extend([2, 2, 0, 8, 128 + 9, 1]);
        assert!(read(&overrun).is_err(), "a run longer than the scanline");
    }
}
