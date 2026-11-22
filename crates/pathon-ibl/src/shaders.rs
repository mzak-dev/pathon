pub const SAMPLE_MODULE: &str = "ibl::sample";
pub const SHADING_MODULE: &str = "ibl::shading";

const SAMPLE_WESL: &str = include_str!("../wgsl/sample.wesl");
const SHADING_WESL: &str = include_str!("../wgsl/shading.wesl");

/// The WESL modules of the ibl shaders as `(path, source)` for `ShaderLibrary::add_module` or
/// `BackgroundCompiler::add_module`; shaders import them as `package::ibl::sample` and `package::ibl::shading`.
pub fn modules() -> [(&'static str, &'static str); 2] {
    [(SAMPLE_MODULE, SAMPLE_WESL), (SHADING_MODULE, SHADING_WESL)]
}
