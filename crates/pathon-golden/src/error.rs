use std::path::PathBuf;

use pathon_rhi::{Format, RhiError};

use crate::compare::{Comparison, Tolerance};
use crate::store::{UPDATE_ENV, VARIANT_ENV};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GoldenError {
    #[error("golden name {0:?} is invalid: use ASCII letters, digits, '_' and '-' only")]
    InvalidName(String),
    #[error("image {width}x{height} cannot be built from {bytes} bytes of RGBA8 (both sides must be non-zero)")]
    InvalidImage { width: u32, height: u32, bytes: usize },
    #[error("cannot read back a {0:?} texture: golden images take 8-bit RGBA or BGRA 2D textures with one layer")]
    UnsupportedReadback(Format),
    #[error("{path}: {message}")]
    File { path: PathBuf, message: String },
    #[error(
        "no reference image at {path}\n\
         first run? create it from the current output with {UPDATE_ENV}=1 and review the PNG before committing"
    )]
    MissingReference { path: PathBuf },
    #[error(
        "size differs: reference is {}x{}, actual is {}x{}{}\n\
         if the resolution changed on purpose, regenerate the reference with {UPDATE_ENV}=1",
        expected.0, expected.1, actual.0, actual.1, saved_actual_note(actual_artifact.as_ref())
    )]
    SizeMismatch {
        expected: (u32, u32),
        actual: (u32, u32),
        actual_artifact: Option<PathBuf>,
    },
    #[error("{0}")]
    Mismatch(Box<Mismatch>),
    #[error(transparent)]
    Rhi(#[from] RhiError),
}

fn saved_actual_note(artifact: Option<&PathBuf>) -> String {
    artifact.map_or_else(String::new, |path| format!(" (actual saved to {})", path.display()))
}

#[derive(Debug)]
pub struct Mismatch {
    pub name: String,
    pub comparison: Comparison,
    pub tolerance: Tolerance,
    pub reference: PathBuf,
    pub artifacts: Artifacts,
}

#[derive(Debug)]
pub struct Artifacts {
    pub actual: PathBuf,
    pub expected: PathBuf,
    pub diff: PathBuf,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            name,
            comparison,
            tolerance,
            reference,
            artifacts,
        } = self;
        writeln!(f, "golden image {name:?} does not match its reference")?;
        writeln!(
            f,
            "  {} of {} pixels differ ({:.3}%), the tolerance allows {:.3}% with per-channel delta up to {}",
            comparison.differing_pixels,
            comparison.total_pixels,
            comparison.differing_fraction() * 100.0,
            f64::from(tolerance.max_differing_fraction) * 100.0,
            tolerance.max_channel_delta,
        )?;
        writeln!(f, "  largest per-channel delta: {}", comparison.max_channel_delta)?;
        if let Some(first) = comparison.first_difference {
            writeln!(
                f,
                "  first differing pixel ({}, {}): reference {:?}, actual {:?}",
                first.x, first.y, first.expected, first.actual
            )?;
        }
        writeln!(f, "  reference: {}", reference.display())?;
        writeln!(f, "  actual:    {}", artifacts.actual.display())?;
        writeln!(
            f,
            "  diff:      {} (red = beyond tolerance, blue = within it)",
            artifacts.diff.display()
        )?;
        write!(
            f,
            "  intended change? regenerate with {UPDATE_ENV}=1 (per backend: {VARIANT_ENV}=<name>) and review the PNG"
        )
    }
}
