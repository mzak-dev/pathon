// M4-10: the viewer of the test scene. `upload` hands a `LoadedScene` to the engine crates, `pack` is the one
// `RenderFeature` that draws it, `session` binds both to a renderer, and `window` / `screenshot` are the two ways to
// run it (docs/examples.md).

pub mod camera;
pub mod controls;
mod gpu_data;
pub mod pack;
pub mod screenshot;
pub mod session;
pub mod settings;
pub mod upload;
pub mod window;
