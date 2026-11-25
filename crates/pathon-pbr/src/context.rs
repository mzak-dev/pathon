use std::collections::HashMap;
use std::sync::Arc;

use pathon_api::glam::Mat4;
use pathon_api::resources::DeferredRelease;
use pathon_api::rhi::{
    DeviceApi, Extent3d, Format, RhiError, SamplerDesc, TexelLayout, TextureCopy, TextureDesc, TextureUsage,
    TextureViewDesc,
};
use pathon_api::{Diagnostic, RecordCtx, ViewId};
use pathon_bindless::TextureBinder;
use pathon_ibl::{IblEnvironment, IblTexture};
use pathon_material::MaterialHooks;

use crate::arena::{ArenaSlice, BufferArena};
use crate::cluster::{ClusterGrid, ClusterPipeline};
use crate::draw::SceneDraws;
use crate::frame::{ViewHistory, ViewMatrices};
use crate::light::PackedLights;
use crate::settings::PbrSettings;
use crate::shaders::{PbrShaders, TEXTURE_GROUP};

/// What the prepass prepared for a view in one record attempt and the colour pass consumes.
#[derive(Clone, Debug)]
pub struct ViewFrame {
    pub record_epoch: u64,
    pub draws: Arc<SceneDraws>,
    pub lights: PackedLights,
    pub grid: ClusterGrid,
    pub settings: PbrSettings,
    pub matrices: ViewMatrices,
    pub frame_uniforms: ArenaSlice,
    pub instances: ArenaSlice,
    pub light_array: ArenaSlice,
    pub materials: pathon_api::rhi::BufferId,
}

/// The state the features of `PbrPack` share, kept in `Shared`: shaders and pipelines, the per-record scratch buffer,
/// the cluster pipeline and the per-view hand-over from the prepass to the colour pass.
#[derive(Debug, Default)]
pub struct PbrContext {
    pub(crate) shaders: Option<PbrShaders>,
    shader_fingerprint: Option<String>,
    pub(crate) cluster: Option<ClusterPipeline>,
    pub(crate) arena: BufferArena,
    pub(crate) views: HashMap<ViewId, ViewFrame>,
    pub(crate) histories: HashMap<ViewId, ViewHistory>,
    pub(crate) material_release: Option<DeferredRelease>,
    pub(crate) placeholder: Option<IblEnvironment>,
    pub(crate) reported: Reported,
}

/// Problems that are reported once, not every frame.
#[derive(Debug, Default)]
pub(crate) struct Reported {
    pub(crate) missing_inputs: bool,
    pub(crate) settings: bool,
    pub(crate) skipped: bool,
}

impl PbrContext {
    /// The shaders for the current hooks and texture path; rebuilt when they change (a new `TextureBinder`).
    pub(crate) fn shaders(
        &mut self,
        hooks: Option<&MaterialHooks>,
        binder: Option<&TextureBinder>,
    ) -> Result<&mut PbrShaders, pathon_shader::ShaderError> {
        let mut hooks = hooks.cloned().unwrap_or_default();
        if let Some(binder) = binder {
            hooks.set_texture_source(binder.texture_source_wgsl(TEXTURE_GROUP));
        }
        let fingerprint: String = hooks.modules().iter().map(|(path, source)| format!("{path}:{source}")).collect();
        if self.shader_fingerprint.as_ref() != Some(&fingerprint) || self.shaders.is_none() {
            self.shaders = Some(PbrShaders::new(&hooks)?);
            self.cluster = None;
            self.shader_fingerprint = Some(fingerprint);
        }
        Ok(self.shaders.as_mut().expect("built above"))
    }

    pub(crate) fn release(&mut self) -> &mut DeferredRelease {
        self.material_release.get_or_insert_with(|| DeferredRelease::new(4))
    }

    /// The environment of the frame, or a black one when the application made none.
    pub(crate) fn environment(
        &mut self,
        device: &dyn DeviceApi,
        published: Option<IblEnvironment>,
    ) -> Result<IblEnvironment, RhiError> {
        if let Some(environment) = published {
            return Ok(environment);
        }
        if let Some(placeholder) = self.placeholder {
            return Ok(placeholder);
        }
        let placeholder = black_environment(device)?;
        self.placeholder = Some(placeholder);
        Ok(placeholder)
    }

    /// After device loss every handle is dead.
    pub(crate) fn forget_gpu(&mut self) {
        self.shaders = None;
        self.shader_fingerprint = None;
        self.cluster = None;
        self.arena.forget_gpu();
        self.views.clear();
        self.placeholder = None;
        if let Some(release) = &mut self.material_release {
            release.forget_all();
        }
    }
}

fn black_cube(device: &dyn DeviceApi, label: &'static str) -> Result<IblTexture, RhiError> {
    let texture = device.create_texture(&TextureDesc::new_cube(
        label,
        1,
        Format::Rgba16Float,
        TextureUsage::SAMPLED | TextureUsage::COPY_DST,
    ))?;
    device.write_texture(
        &TextureCopy::whole(texture),
        &[0; 8 * 6],
        TexelLayout {
            offset: 0,
            bytes_per_row: 8,
            rows_per_image: 1,
        },
        Extent3d {
            width: 1,
            height: 1,
            depth_or_layers: 6,
        },
    )?;
    let view = device.create_texture_view(texture, &TextureViewDesc::cube())?;
    Ok(IblTexture { texture, view })
}

// No environment: a black one, so a scene lit only by its lights draws without an IBL pack.
fn black_environment(device: &dyn DeviceApi) -> Result<IblEnvironment, RhiError> {
    let table = {
        let texture = device.create_texture(&TextureDesc::new_2d(
            "pbr black brdf lut",
            1,
            1,
            Format::Rgba16Float,
            TextureUsage::SAMPLED | TextureUsage::COPY_DST,
        ))?;
        device.write_texture(
            &TextureCopy::whole(texture),
            &[0; 8],
            TexelLayout {
                offset: 0,
                bytes_per_row: 8,
                rows_per_image: 1,
            },
            Extent3d::new_2d(1, 1),
        )?;
        IblTexture {
            texture,
            view: device.create_texture_view(texture, &TextureViewDesc::default())?,
        }
    };
    Ok(IblEnvironment {
        irradiance: black_cube(device, "pbr black irradiance")?,
        prefiltered: black_cube(device, "pbr black prefiltered")?,
        brdf_lut: table,
        sampler: device.create_sampler(&SamplerDesc::LINEAR_CLAMP)?,
        prefiltered_levels: 1,
        generation: 0,
    })
}

/// Reports a diagnostic once per `flag`.
pub(crate) fn report_once(ctx: &mut RecordCtx<'_>, flag: &mut bool, diagnostic: Diagnostic) {
    if !*flag {
        *flag = true;
        ctx.report(diagnostic);
    }
}

/// The matrix a view had one committed frame ago.
pub(crate) fn previous_view_projection(
    histories: &mut HashMap<ViewId, ViewHistory>,
    view: ViewId,
    frame: u64,
    current: Mat4,
) -> Mat4 {
    histories
        .entry(view)
        .or_insert_with(|| ViewHistory::first(frame, current))
        .advance(frame, current)
}
