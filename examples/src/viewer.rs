// M4-10: the viewer of the test scene. `upload` hands a `LoadedScene` to the engine crates, `stack` assembles the packs
// that light it (`simple` is the one-feature stand-in kept for `--simple`), `session` binds both to a renderer, and
// `window` / `screenshot` are the two ways to run it (docs/examples.md).

pub mod bench;
pub mod camera;
pub mod controls;
pub mod environment;
mod gpu_data;
pub mod screenshot;
pub mod session;
pub mod settings;
pub mod simple;
pub mod sky;
pub mod stack;
pub mod upload;
pub mod window;
