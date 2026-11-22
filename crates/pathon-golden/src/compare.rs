use crate::error::GoldenError;
use crate::image::{GoldenImage, pixel_count};

/// A pixel differs when any channel is further than `max_channel_delta` from the reference; the image passes when
/// at most `max_differing_fraction` of its pixels differ (ADR-0025).
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct Tolerance {
    pub max_channel_delta: u8,
    pub max_differing_fraction: f32,
}

impl Tolerance {
    pub const fn new(max_channel_delta: u8, max_differing_fraction: f32) -> Self {
        Self {
            max_channel_delta,
            max_differing_fraction,
        }
    }

    /// Every pixel identical: the same backend and driver, or a CPU-only computation.
    pub const fn exact() -> Self {
        Self::new(0, 0.0)
    }

    /// Absorbs rounding and edge-coverage differences between rasterizers (lavapipe, WARP, Metal, hardware).
    pub const fn cross_backend() -> Self {
        Self::new(6, 0.03)
    }

    pub(crate) fn admits(self, comparison: &Comparison) -> bool {
        // Same precision on both sides, so that 1 of 100 pixels is exactly within 0.01.
        comparison.differing_pixels as f32 / comparison.total_pixels as f32 <= self.max_differing_fraction
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PixelDifference {
    pub x: u32,
    pub y: u32,
    pub expected: [u8; 4],
    pub actual: [u8; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Comparison {
    pub total_pixels: usize,
    pub differing_pixels: usize,
    pub max_channel_delta: u8,
    pub first_difference: Option<PixelDifference>,
}

impl Comparison {
    pub fn differing_fraction(&self) -> f64 {
        self.differing_pixels as f64 / self.total_pixels as f64
    }

    pub fn is_identical(&self) -> bool {
        self.max_channel_delta == 0
    }
}

pub fn compare(expected: &GoldenImage, actual: &GoldenImage, tolerance: Tolerance) -> Result<Comparison, GoldenError> {
    ensure_same_size(expected, actual)?;
    let mut comparison = Comparison {
        total_pixels: pixel_count(expected.width(), expected.height()),
        differing_pixels: 0,
        max_channel_delta: 0,
        first_difference: None,
    };
    for (index, (want, got)) in expected.pixels().zip(actual.pixels()).enumerate() {
        let delta = channel_delta(want, got);
        comparison.max_channel_delta = comparison.max_channel_delta.max(delta);
        if delta > tolerance.max_channel_delta {
            comparison.differing_pixels += 1;
            comparison.first_difference.get_or_insert_with(|| {
                let (x, y) = coordinates(index, expected.width());
                PixelDifference {
                    x,
                    y,
                    expected: want,
                    actual: got,
                }
            });
        }
    }
    Ok(comparison)
}

/// Dark copy of the reference where pixels agree, blue where they differ within the tolerance, red beyond it.
pub fn diff_image(
    expected: &GoldenImage,
    actual: &GoldenImage,
    tolerance: Tolerance,
) -> Result<GoldenImage, GoldenError> {
    ensure_same_size(expected, actual)?;
    let mut rgba = Vec::with_capacity(expected.as_rgba8().len());
    for (want, got) in expected.pixels().zip(actual.pixels()) {
        let delta = channel_delta(want, got);
        rgba.extend_from_slice(&match delta {
            0 => dimmed_luma(want),
            delta if delta > tolerance.max_channel_delta => [255, 0, 0, 255],
            _ => [0, 128, 255, 255],
        });
    }
    GoldenImage::from_rgba8(expected.width(), expected.height(), rgba)
}

pub(crate) fn ensure_same_size(expected: &GoldenImage, actual: &GoldenImage) -> Result<(), GoldenError> {
    if (expected.width(), expected.height()) == (actual.width(), actual.height()) {
        return Ok(());
    }
    Err(GoldenError::SizeMismatch {
        expected: (expected.width(), expected.height()),
        actual: (actual.width(), actual.height()),
        actual_artifact: None,
    })
}

fn channel_delta(expected: [u8; 4], actual: [u8; 4]) -> u8 {
    expected
        .iter()
        .zip(actual)
        .map(|(want, got)| want.abs_diff(got))
        .max()
        .unwrap_or(0)
}

fn coordinates(index: usize, width: u32) -> (u32, u32) {
    let width = width as usize;
    // The indices come from an image whose dimensions are u32.
    (
        u32::try_from(index % width).unwrap_or(u32::MAX),
        u32::try_from(index / width).unwrap_or(u32::MAX),
    )
}

fn dimmed_luma(pixel: [u8; 4]) -> [u8; 4] {
    let sum = u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2]);
    let dimmed = u8::try_from(sum / 9).unwrap_or(u8::MAX);
    [dimmed, dimmed, dimmed, 255]
}
