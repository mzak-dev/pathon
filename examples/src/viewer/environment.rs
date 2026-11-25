// The environment picture of the pbr viewer: a Radiance `.hdr` file next to the scene (`env/Garage.hdr` in the
// classroom folder, docs/scenes.md) or a flag; without one the stack uses a synthetic sky.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pathon_ibl::EquirectImage;

use crate::radiance;

const DEFAULT_RELATIVE_PATH: &str = "env/Garage.hdr";

/// `env/Garage.hdr` in the folder of `scene`, when it exists.
pub fn default_environment_path(scene: &Path) -> Option<PathBuf> {
    let candidate = scene.parent()?.join(DEFAULT_RELATIVE_PATH);
    candidate.is_file().then_some(candidate)
}

pub fn load_environment(path: &Path) -> Result<Arc<EquirectImage>, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let picture = radiance::read(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    let image = EquirectImage::from_rgb_f32(picture.width, picture.height, &picture.rgb)
        .map_err(|error| format!("{}: {}", path.display(), error.to_diagnostic()))?;
    Ok(Arc::new(image))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scene_without_an_env_folder_has_no_default_environment() {
        assert_eq!(default_environment_path(Path::new("no-such-folder/scene.glb")), None);
    }
}
