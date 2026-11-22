use std::path::{Path, PathBuf};
use std::{env, fs};

use crate::compare::{Comparison, Tolerance, compare, diff_image, ensure_same_size};
use crate::error::{Artifacts, GoldenError, Mismatch};
use crate::image::GoldenImage;
use crate::png_io::{read_png, write_png};

pub(crate) const UPDATE_ENV: &str = "PATHON_UPDATE_GOLDEN";
pub(crate) const VARIANT_ENV: &str = "PATHON_GOLDEN_VARIANT";
pub(crate) const DIR_ENV: &str = "PATHON_GOLDEN_DIR";
pub(crate) const ARTIFACTS_ENV: &str = "PATHON_GOLDEN_ARTIFACTS";

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Matched(Comparison),
    Created(PathBuf),
    Updated(PathBuf),
}

/// Where references live, where failure artifacts go, and whether a run rewrites references.
#[derive(Clone, Debug)]
pub struct GoldenStore {
    reference_dir: PathBuf,
    artifacts_dir: PathBuf,
    variant: Option<String>,
    update: bool,
}

impl GoldenStore {
    pub fn new(reference_dir: impl Into<PathBuf>, artifacts_dir: impl Into<PathBuf>) -> Self {
        Self {
            reference_dir: reference_dir.into(),
            artifacts_dir: artifacts_dir.into(),
            variant: None,
            update: false,
        }
    }

    /// `tests/golden/` and `target/golden-artifacts/` in the workspace, overridable with `PATHON_GOLDEN_DIR` and
    /// `PATHON_GOLDEN_ARTIFACTS`; `PATHON_UPDATE_GOLDEN=1` rewrites references, `PATHON_GOLDEN_VARIANT=<name>`
    /// selects the per-backend reference directory.
    pub fn from_env() -> Self {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap_or(Path::new("."));
        let reference_dir = env::var_os(DIR_ENV).map_or_else(|| workspace.join("tests/golden"), PathBuf::from);
        let artifacts_dir =
            env::var_os(ARTIFACTS_ENV).map_or_else(|| workspace.join("target/golden-artifacts"), PathBuf::from);
        let mut store = Self::new(reference_dir, artifacts_dir).with_update(env_flag(UPDATE_ENV));
        store.variant = env::var(VARIANT_ENV).ok().filter(|variant| !variant.is_empty());
        store
    }

    /// References in `<dir>/<variant>/` take precedence over the shared ones in `<dir>/` (ADR-0025).
    #[must_use]
    pub fn with_variant(mut self, variant: impl Into<String>) -> Self {
        self.variant = Some(variant.into());
        self
    }

    #[must_use]
    pub fn with_update(mut self, update: bool) -> Self {
        self.update = update;
        self
    }

    pub fn check(&self, name: &str, actual: &GoldenImage, tolerance: Tolerance) -> Result<Outcome, GoldenError> {
        validate_name(name)?;
        if let Some(variant) = &self.variant {
            validate_name(variant)?;
        }
        let reference = self.existing_reference(name);
        if self.update {
            return self.update_reference(name, actual, reference.as_deref());
        }
        let Some(reference) = reference else {
            return Err(GoldenError::MissingReference {
                path: self.writable_reference_path(name),
            });
        };
        let expected = read_png(&reference)?;
        if let Err(GoldenError::SizeMismatch {
            expected: reference_size,
            actual: actual_size,
            ..
        }) = ensure_same_size(&expected, actual)
        {
            let saved = self.artifact_path(name, "actual");
            write_png(&saved, actual)?;
            return Err(GoldenError::SizeMismatch {
                expected: reference_size,
                actual: actual_size,
                actual_artifact: Some(saved),
            });
        }
        let comparison = compare(&expected, actual, tolerance)?;
        if tolerance.admits(&comparison) {
            self.remove_stale_artifacts(name);
            return Ok(Outcome::Matched(comparison));
        }
        let artifacts = self.write_artifacts(name, &expected, actual, tolerance)?;
        Err(GoldenError::Mismatch(Box::new(Mismatch {
            name: name.to_owned(),
            comparison,
            tolerance,
            reference,
            artifacts,
        })))
    }

    fn update_reference(
        &self,
        name: &str,
        actual: &GoldenImage,
        existing: Option<&Path>,
    ) -> Result<Outcome, GoldenError> {
        let target = self.writable_reference_path(name);
        let unchanged = existing
            .and_then(|path| read_png(path).ok())
            .filter(|image| image == actual);
        if let Some(image) = unchanged {
            let comparison = compare(&image, actual, Tolerance::exact())?;
            return Ok(Outcome::Matched(comparison));
        }
        let created = !target.is_file();
        write_png(&target, actual)?;
        Ok(if created {
            Outcome::Created(target)
        } else {
            Outcome::Updated(target)
        })
    }

    fn write_artifacts(
        &self,
        name: &str,
        expected: &GoldenImage,
        actual: &GoldenImage,
        tolerance: Tolerance,
    ) -> Result<Artifacts, GoldenError> {
        let artifacts = Artifacts {
            actual: self.artifact_path(name, "actual"),
            expected: self.artifact_path(name, "expected"),
            diff: self.artifact_path(name, "diff"),
        };
        write_png(&artifacts.actual, actual)?;
        write_png(&artifacts.expected, expected)?;
        write_png(&artifacts.diff, &diff_image(expected, actual, tolerance)?)?;
        Ok(artifacts)
    }

    fn remove_stale_artifacts(&self, name: &str) {
        for kind in ["actual", "expected", "diff"] {
            // A leftover file is only noise, so a failed removal is not worth an error.
            drop(fs::remove_file(self.artifact_path(name, kind)));
        }
    }

    fn artifact_path(&self, name: &str, kind: &str) -> PathBuf {
        self.artifacts_dir.join(format!("{name}.{kind}.png"))
    }

    fn existing_reference(&self, name: &str) -> Option<PathBuf> {
        let shared = self.reference_dir.join(format!("{name}.png"));
        let variant = self
            .variant
            .as_ref()
            .map(|variant| self.reference_dir.join(variant).join(format!("{name}.png")));
        variant
            .filter(|path| path.is_file())
            .or_else(|| shared.is_file().then_some(shared))
    }

    fn writable_reference_path(&self, name: &str) -> PathBuf {
        match &self.variant {
            Some(variant) => self.reference_dir.join(variant).join(format!("{name}.png")),
            None => self.reference_dir.join(format!("{name}.png")),
        }
    }
}

fn validate_name(name: &str) -> Result<(), GoldenError> {
    let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if valid {
        Ok(())
    } else {
        Err(GoldenError::InvalidName(name.to_owned()))
    }
}

fn env_flag(name: &str) -> bool {
    env::var(name).is_ok_and(|value| !matches!(value.as_str(), "" | "0" | "false"))
}

/// Compares `image` with `tests/golden/<name>.png` and panics with the full report when it does not match.
///
/// `PATHON_UPDATE_GOLDEN=1` creates or rewrites the reference instead.
#[track_caller]
pub fn assert_golden(name: &str, image: &GoldenImage, tolerance: Tolerance) {
    match GoldenStore::from_env().check(name, image, tolerance) {
        Ok(Outcome::Matched(_)) => {}
        Ok(Outcome::Created(path)) => report(&format!("golden {name:?}: created {}", path.display())),
        Ok(Outcome::Updated(path)) => report(&format!("golden {name:?}: updated {}", path.display())),
        Err(error) => panic!("{error}"),
    }
}

#[expect(
    clippy::print_stderr,
    reason = "update mode tells the developer which reference changed"
)]
fn report(message: &str) {
    eprintln!("{message}");
}
