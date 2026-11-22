use pathon_shader::{ShaderComposer, ShaderError, ShaderFeatures, reflect_bind_groups};

use crate::MaterialError;
use crate::variant::flags;

pub const SURFACE_MODULE: &str = "material::surface";
pub const STANDARD_MODULE: &str = "material::standard";
pub const HOOKS_MODULE: &str = "material::hooks";
pub const TEXTURE_SOURCE_MODULE: &str = "material::texture_source";

const SURFACE_WESL: &str = include_str!("../wgsl/surface.wesl");
const STANDARD_WESL: &str = include_str!("../wgsl/standard.wesl");
const HOOKS_WESL: &str = include_str!("../wgsl/hooks.wesl");
const TEXTURE_SOURCE_WESL: &str = include_str!("../wgsl/texture_source.wesl");

const PROBE: &str = "
import package::material::standard::{MaterialData, evaluate, load_surface};
import package::material::surface::{LightSample, SurfaceInput};

@group(0) @binding(0) var<storage, read> materials: array<MaterialData>;
@group(0) @binding(1) var<storage, read_write> probe_output: array<vec4<f32>>;

@compute @workgroup_size(1)
fn probe() {
    var input: SurfaceInput;
    input.normal = vec3<f32>(0.0, 0.0, 1.0);
    input.tangent = vec4<f32>(1.0, 0.0, 0.0, 1.0);
    input.front_facing = true;
    let surface = load_surface(materials[0], input);
    let result = evaluate(surface, input.normal, LightSample(input.normal, vec3<f32>(1.0)));
    probe_output[0] = vec4<f32>(result.diffuse + result.specular, 1.0);
}
";

/// Modules a pack can replace to extend the standard material in WGSL: the hook modules keep the signature of their
/// default and may import the rest of `package::material::*`.
///
/// Packs share one value through `Shared` (`get_or_insert_with(MaterialHooks::default)`); whoever owns the
/// `ShaderLibrary` or `BackgroundCompiler` registers [`Self::modules`] in it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct MaterialHooks {
    surface: Option<String>,
    texture_source: Option<String>,
}

impl MaterialHooks {
    /// Replaces `package::material::hooks`: `fn modify_surface(input: SurfaceInput, surface: Surface) -> Surface`,
    /// called at the end of `load_surface`.
    pub fn set_surface_hook(&mut self, source: impl Into<String>) -> &mut Self {
        self.surface = Some(source.into());
        self
    }

    /// Replaces `package::material::texture_source`: `fn sample_material_texture(slot: u32, uv: vec2<f32>,
    /// uv_dx: vec2<f32>, uv_dy: vec2<f32>) -> vec4<f32>`, the only place the material reads a texture.
    pub fn set_texture_source(&mut self, source: impl Into<String>) -> &mut Self {
        self.texture_source = Some(source.into());
        self
    }

    /// Every module of the material shaders as `(path, source)` for `add_module`, with the hooks applied.
    pub fn modules(&self) -> [(&'static str, &str); 4] {
        [
            (SURFACE_MODULE, SURFACE_WESL),
            (STANDARD_MODULE, STANDARD_WESL),
            (HOOKS_MODULE, self.surface.as_deref().unwrap_or(HOOKS_WESL)),
            (
                TEXTURE_SOURCE_MODULE,
                self.texture_source.as_deref().unwrap_or(TEXTURE_SOURCE_WESL),
            ),
        ]
    }

    /// Composes and validates the modules with no flag and with every flag enabled, so a hook that breaks one
    /// permutation fails here and not on the first draw (`Y0003`).
    pub fn validate(&self) -> Result<(), MaterialError> {
        let invalid = |error: ShaderError| MaterialError::InvalidHook {
            message: error.to_string(),
        };
        let mut composer = ShaderComposer::new();
        for (path, source) in self.modules() {
            composer.add_module(path, source).map_err(invalid)?;
        }
        let mut every_flag = ShaderFeatures::default();
        for flag in [
            flags::MASKED,
            flags::TRANSPARENT,
            flags::SUBSURFACE,
            flags::TRANSMISSION,
            flags::CLEARCOAT,
            flags::BURLEY,
        ] {
            every_flag.set(flag, true).map_err(invalid)?;
        }
        for features in [ShaderFeatures::default(), every_flag] {
            let wgsl = composer
                .compose_with(PROBE, "material probe", &features)
                .map_err(invalid)?;
            reflect_bind_groups(&wgsl, &["probe"]).map_err(invalid)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_modules_compose_and_validate() {
        assert_eq!(MaterialHooks::default().validate(), Ok(()));
    }

    #[test]
    fn a_hook_with_the_wrong_signature_is_y0003() {
        let mut hooks = MaterialHooks::default();
        hooks.set_surface_hook("fn modify_surface(x: f32) -> f32 { return x; }");

        let error = hooks.validate().expect_err("the signature is wrong");

        assert_eq!(error.code(), "Y0003");
    }
}
