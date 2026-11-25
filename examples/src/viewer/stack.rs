// Which packs the viewer builds its renderer from, and how the viewer's settings reach them each frame.
//
// The pbr stack is what an own engine would assemble: `IblPack` turns the environment picture into light, `ShadowPack`
// draws the cascades of the sun, `PbrPack` lights the scene (prepass, clustered lights, standard material), the
// viewer's `SkyFeature` paints the backdrop and `PostPack` does exposure, bloom and tone mapping. `--simple` keeps the
// one-feature stand-in of M4-10 for comparison.

use std::sync::Arc;

use pathon::Renderer;
use pathon::RendererBuilder;
use pathon::api::glam::Vec3;
use pathon::rhi::{DeviceRequest, Features};
use pathon_bindless::{BINDLESS_FEATURES, PathPreference};
use pathon_ibl::{EquirectImage, IblPack, IblSettings, IblSource, SyntheticSky};
use pathon_pbr::{LightAssignment, PbrPack, PbrSettings};
use pathon_post::{ExposureMode, PostPack, PostSettings, Tonemapper};
use pathon_shadow::{DirectionalShadowLight, ShadowPack, ShadowScene, ShadowSettings};

use super::settings::{LightingMode, ViewerSettings};
use super::simple::SimplePack;
use super::sky::SkyFeature;
use super::upload::ViewerAssets;

const SYNTHETIC_SKY_SIZE: (u32, u32) = (512, 256);
/// The sun is the only directional light of the classroom, and directional lights come first in the light array of
/// `PbrPack`, so it is light 0 there whatever its place in the file.
const SUN_LIGHT_INDEX: u32 = 0;
/// Shadows beyond this distance from the camera are not drawn: the classroom is 10 m across.
const SHADOW_DISTANCE: f32 = 40.0;
const SHADOW_RESOLUTION: u32 = 2048;

#[derive(Clone, Debug)]
pub struct ViewerConfig {
    pub lighting: LightingMode,
    /// The environment picture; `None` makes a synthetic sky with a sun.
    pub environment: Option<Arc<EquirectImage>>,
    pub texture_path: PathPreference,
    pub tonemapper: Tonemapper,
    /// Asks the device for timestamp queries, which `--bench` needs to time the passes.
    pub gpu_timestamps: bool,
    /// Forces a backend (the tests use the null device); `None` picks the best adapter.
    pub backend: Option<pathon::rhi::Backend>,
}

impl Default for ViewerConfig {
    fn default() -> Self {
        Self {
            lighting: LightingMode::Pbr,
            environment: None,
            texture_path: PathPreference::Auto,
            tonemapper: Tonemapper::AcesFitted,
            gpu_timestamps: false,
            backend: None,
        }
    }
}

fn synthetic_environment() -> Arc<EquirectImage> {
    let (width, height) = SYNTHETIC_SKY_SIZE;
    Arc::new(
        SyntheticSky::default()
            .image(width, height)
            .expect("the synthetic sky has a valid size"),
    )
}

impl ViewerConfig {
    pub fn builder(&self) -> RendererBuilder {
        let mut features = if self.gpu_timestamps {
            Features::TIMESTAMP_QUERY
        } else {
            Features::empty()
        };
        if self.lighting == LightingMode::Pbr {
            features |= BINDLESS_FEATURES;
        }
        let builder = Renderer::builder().with_device_request(DeviceRequest {
            label: "gltf viewer",
            backend: self.backend,
            optional_features: features,
            ..DeviceRequest::default()
        });
        if self.lighting == LightingMode::Simple {
            return builder.with_feature(SimplePack::new());
        }
        let environment = self.environment.clone().unwrap_or_else(synthetic_environment);
        let post = PostSettings {
            tonemap: pathon_post::TonemapSettings {
                operator: self.tonemapper,
                ..pathon_post::TonemapSettings::default()
            },
            ..PostSettings::default()
        };
        builder
            .with_pack(IblPack::new())
            .with_shared(IblSource::new(environment, IblSettings::DESKTOP))
            .with_pack(ShadowPack::with_settings(ShadowSettings {
                max_distance: SHADOW_DISTANCE,
                resolution: SHADOW_RESOLUTION,
                ..ShadowSettings::DESKTOP
            }))
            .with_pack(PbrPack::new())
            .with_feature(SkyFeature::new())
            .with_pack(PostPack::with_settings(post))
    }
}

/// Fills what the shadow pack renders: the casters of the scene, once; whether the sun shines through them follows
/// `ViewerSettings::sun_shadows` every frame.
pub fn fill_shadow_casters(renderer: &mut Renderer, assets: &ViewerAssets) {
    if let Some(scene) = renderer.shared_mut().get_mut::<ShadowScene>() {
        scene.clear_casters();
        for caster in &assets.shadow_casters {
            scene.add_caster(*caster);
        }
    }
}

/// Pushes the viewer's settings into the packs' own settings before a frame.
pub fn apply_settings(renderer: &mut Renderer, settings: &ViewerSettings, sun_direction: Option<Vec3>) {
    let shared = renderer.shared_mut();
    shared.insert(*settings);
    if let Some(pbr) = shared.get_mut::<PbrSettings>() {
        pbr.environment_intensity = settings.environment_intensity;
        pbr.light_assignment = if settings.brute_force_lights {
            LightAssignment::BruteForce
        } else {
            LightAssignment::Clustered
        };
    }
    if let Some(scene) = shared.get_mut::<ShadowScene>() {
        scene.light = sun_direction
            .filter(|_| settings.sun_shadows)
            .map(|direction| DirectionalShadowLight::new(direction, SUN_LIGHT_INDEX));
    }
    if let Some(post) = shared.get_mut::<PostSettings>() {
        post.bloom.enabled = settings.bloom;
        if settings.auto_exposure {
            if !matches!(post.exposure.mode, ExposureMode::Auto(_)) {
                post.exposure.mode = ExposureMode::default();
            }
            post.exposure.compensation_ev = settings.exposure_stops();
        } else {
            post.exposure.mode = ExposureMode::Manual {
                ev: -settings.exposure.log2(),
            };
            post.exposure.compensation_ev = 0.0;
        }
    }
}
