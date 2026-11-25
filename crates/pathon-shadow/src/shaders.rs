/// Path of the cascaded shadow sampling module as `ShaderLibrary::add_module` takes it; shaders import it as
/// `package::shadow::sample`.
pub const SAMPLE_MODULE: &str = "shadow::sample";

const SAMPLE_WESL: &str = include_str!("../wgsl/sample.wesl");

/// The WESL modules of the shadow shaders as `(path, source)` for `ShaderLibrary::add_module` or
/// `BackgroundCompiler::add_module`.
pub fn modules() -> [(&'static str, &'static str); 1] {
    [(SAMPLE_MODULE, SAMPLE_WESL)]
}
