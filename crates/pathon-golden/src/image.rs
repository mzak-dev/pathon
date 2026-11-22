use crate::error::GoldenError;

const CHANNELS: usize = 4;

/// Tightly packed RGBA8 image, row 0 at the top.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GoldenImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl GoldenImage {
    pub fn from_rgba8(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, GoldenError> {
        let expected = pixel_count(width, height) * CHANNELS;
        if width == 0 || height == 0 || rgba.len() != expected {
            return Err(GoldenError::InvalidImage {
                width,
                height,
                bytes: rgba.len(),
            });
        }
        Ok(Self { width, height, rgba })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn as_rgba8(&self) -> &[u8] {
        &self.rgba
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let start = (y as usize * self.width as usize + x as usize) * CHANNELS;
        [
            self.rgba[start],
            self.rgba[start + 1],
            self.rgba[start + 2],
            self.rgba[start + 3],
        ]
    }

    pub(crate) fn pixels(&self) -> impl Iterator<Item = [u8; 4]> + '_ {
        self.rgba.as_chunks::<CHANNELS>().0.iter().copied()
    }
}

pub(crate) fn pixel_count(width: u32, height: u32) -> usize {
    width as usize * height as usize
}
