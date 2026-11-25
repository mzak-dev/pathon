// What the windowed viewer and the screenshot mode share: the renderer with the uploaded scene, the free camera and
// the settings.

use std::sync::Arc;
use std::time::Instant;

use pathon::api::Camera;
use pathon::{FrameInput, FrameReport, RenderError, Renderer, TargetId, ViewInput};

use super::camera::{FlyCamera, overview};
use super::settings::{LightingMode, ViewerSettings};
use super::stack::{ViewerConfig, apply_settings, fill_shadow_casters};
use super::upload::{SceneGpu, SharedSimple, UploadError, ViewerAssets, upload_scene};
use crate::gltf_loader::LoadedScene;

const DEFAULT_VERTICAL_FOV: f32 = std::f32::consts::FRAC_PI_3;
// A walk of this many seconds crosses the scene's diagonal.
const SECONDS_TO_CROSS_THE_SCENE: f32 = 10.0;

#[derive(Debug)]
pub struct Session {
    pub renderer: Renderer,
    pub assets: Arc<ViewerAssets>,
    pub camera: FlyCamera,
    pub settings: ViewerSettings,
    pub lighting: LightingMode,
    started: Instant,
    last_frame: Instant,
}

impl Session {
    /// Uploads `scene` through the device of `renderer`, which must have been built by `config.builder()`.
    pub fn new(mut renderer: Renderer, scene: &LoadedScene, config: &ViewerConfig) -> Result<Self, UploadError> {
        let started = Instant::now();
        let uploaded = upload_scene(renderer.device().as_ref(), scene, config.lighting, config.texture_path)?;
        let assets = Arc::new(uploaded.assets);
        let stats = assets.stats;
        log::info!(
            "uploaded in {:.1} s: {} meshes ({} triangles), {} textures, {} materials, {} instances, {} lights",
            started.elapsed().as_secs_f32(),
            stats.meshes,
            stats.triangles,
            stats.textures,
            stats.materials,
            stats.instances,
            stats.lights,
        );
        match uploaded.gpu {
            SceneGpu::Simple(simple) => {
                renderer.shared_mut().insert(SharedSimple(simple));
            }
            SceneGpu::Pbr(pbr) => {
                let shared = renderer.shared_mut();
                shared.insert(pbr.pool);
                shared.insert(pbr.materials);
                shared.insert(pbr.binder);
                shared.insert(Arc::clone(&assets.snapshot));
                fill_shadow_casters(&mut renderer, &assets);
            }
        }
        let (min, max) = assets.bounds;
        let speed = (max - min).length() / SECONDS_TO_CROSS_THE_SCENE;
        let mut camera = overview(assets.bounds, DEFAULT_VERTICAL_FOV);
        camera.speed = speed;
        let mut session = Self {
            renderer,
            assets,
            camera,
            settings: ViewerSettings::default(),
            lighting: config.lighting,
            started: Instant::now(),
            last_frame: Instant::now(),
        };
        for (index, entry) in session.assets.cameras.iter().enumerate() {
            let main = if session.assets.main_camera == Some(index) {
                " (main)"
            } else {
                ""
            };
            log::info!("camera {index}: {}{main}", entry.name);
        }
        if let Some(main) = session.assets.main_camera {
            session.use_scene_camera(main);
        }
        Ok(session)
    }

    /// The `index`-th camera of the file (the number key minus one); `false` when the file has fewer.
    pub fn use_scene_camera(&mut self, index: usize) -> bool {
        let Some(entry) = self.assets.cameras.get(index) else {
            return false;
        };
        let Some(data) = self.assets.snapshot.camera(entry.id) else {
            return false;
        };
        self.camera.jump_to(data);
        true
    }

    pub fn camera_for(&self, aspect_ratio: f32) -> Camera {
        self.camera.camera(aspect_ratio)
    }

    /// Renders one frame of the free camera into `target`; time advances with the wall clock.
    pub fn render(&mut self, target: TargetId, aspect_ratio: f32) -> Result<FrameReport, RenderError> {
        let now = Instant::now();
        let input = FrameInput {
            time_seconds: now.duration_since(self.started).as_secs_f64(),
            delta_seconds: now.duration_since(self.last_frame).as_secs_f32(),
            views: vec![ViewInput {
                target,
                camera: self.camera_for(aspect_ratio),
            }],
        };
        self.last_frame = now;
        self.render_input(&input)
    }

    /// Renders `input` with the current settings; `delta_seconds` is up to the caller (the benchmark fixes it).
    pub fn render_input(&mut self, input: &FrameInput) -> Result<FrameReport, RenderError> {
        apply_settings(&mut self.renderer, &self.settings, self.assets.sun_direction);
        self.renderer.render_frame(input)
    }

    pub fn describe(&self) -> String {
        let position = self.camera.position;
        let exposure = if self.settings.auto_exposure {
            format!("auto {:+.1} EV", self.settings.exposure_stops())
        } else {
            format!("1/{:.0}", 1.0 / self.settings.exposure)
        };
        let light = match self.lighting {
            LightingMode::Simple => format!(
                "simple, ambient {:.0}/{:.0} lx",
                self.settings.ambient_sky_lux, self.settings.ambient_ground_lux
            ),
            LightingMode::Pbr => format!(
                "pbr, environment x{:.0}, sky x{:.0}, bloom {}",
                self.settings.environment_intensity,
                self.settings.sky_intensity,
                if self.settings.bloom { "on" } else { "off" },
            ),
        };
        format!(
            "camera at ({:.2}, {:.2}, {:.2}) fov {:.0} deg, exposure {exposure}, {light}, sun shadows {}",
            position.x,
            position.y,
            position.z,
            self.camera.vertical_fov.to_degrees(),
            if self.settings.sun_shadows { "on" } else { "off" },
        )
    }
}
