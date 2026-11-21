use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use pathon_rhi_types::{
    Access, AdapterInfo, Backend, BindGroupDesc, BindGroupId, BindGroupLayoutDesc, BindGroupLayoutEntry,
    BindGroupLayoutId, BindingResource, BindingType, BufferDesc, BufferId, BufferUsage, Caps, CommandList,
    ComputePipelineDesc, ComputePipelineId, DeviceApi, DeviceType, Extent3d, Features, Format, FormatCaps, FormatKind,
    Limits, Pool, Profile, QuerySetDesc, QuerySetId, RenderPipelineDesc, RenderPipelineId, Resource, RhiError,
    SamplerDesc, SamplerId, ShaderModuleDesc, ShaderModuleId, StorageAccess, SubresourceRange, SurfaceConfig,
    SurfaceFrame, SurfaceId, SurfaceTarget, TexelLayout, TextureCopy, TextureDesc, TextureDimension, TextureId,
    TextureUsage, TextureViewDesc, TextureViewId,
};

use crate::validator::Validator;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct SubresourceState {
    pub(crate) access: Access,
    pub(crate) initialized: bool,
}

#[derive(Debug)]
pub(crate) struct NullTexture {
    pub(crate) desc: TextureDesc,
    pub(crate) subresources: Vec<SubresourceState>,
}

impl NullTexture {
    fn new(desc: TextureDesc) -> Self {
        let count = desc.mip_level_count * desc.array_layers();
        Self {
            desc,
            subresources: vec![SubresourceState::default(); count as usize],
        }
    }

    pub(crate) fn subresource_mut(&mut self, mip: u32, layer: u32) -> &mut SubresourceState {
        let index = mip * self.desc.array_layers() + layer;
        &mut self.subresources[index as usize]
    }

    pub(crate) fn subresource_list(&self, range: SubresourceRange) -> Vec<(u32, u32)> {
        range
            .subresources(self.desc.mip_level_count, self.desc.array_layers())
            .collect()
    }

    // Like `subresource_list`, but a range reaching past the texture is an error instead of being clamped;
    // a `u32::MAX` count means "to the end".
    pub(crate) fn checked_subresources(&self, range: SubresourceRange) -> Result<Vec<(u32, u32)>, RhiError> {
        let (mips, layers) = (self.desc.mip_level_count, self.desc.array_layers());
        let fits = |base: u32, count: u32, total: u32| {
            base < total && (count == u32::MAX || base.saturating_add(count) <= total)
        };
        if fits(range.base_mip, range.mip_count, mips) && fits(range.base_layer, range.layer_count, layers) {
            return Ok(self.subresource_list(range));
        }
        Err(RhiError::validation(
            "R0112",
            format!(
                "texture '{}' has {mips} mips and {layers} layers; {range:?} reaches outside them",
                self.desc.label
            ),
        ))
    }
}

#[derive(Debug)]
pub(crate) struct NullBuffer {
    pub(crate) desc: BufferDesc,
    pub(crate) data: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NullView {
    pub(crate) texture: TextureId,
    pub(crate) range: SubresourceRange,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum BindingUse {
    Texture {
        view: TextureViewId,
        access: Access,
        reads: bool,
    },
    Buffer,
    Sampler,
}

#[derive(Debug)]
pub(crate) struct NullSurface {
    config: Option<SurfaceConfig>,
    frame: Option<TextureId>,
}

#[derive(Debug, Default)]
pub(crate) struct State {
    pub(crate) buffers: Pool<BufferId, NullBuffer>,
    pub(crate) textures: Pool<TextureId, NullTexture>,
    pub(crate) views: Pool<TextureViewId, NullView>,
    samplers: Pool<SamplerId, ()>,
    modules: Pool<ShaderModuleId, ()>,
    layouts: Pool<BindGroupLayoutId, BindGroupLayoutDesc>,
    pub(crate) bind_groups: Pool<BindGroupId, Vec<BindingUse>>,
    pub(crate) render_pipelines: Pool<RenderPipelineId, ()>,
    pub(crate) compute_pipelines: Pool<ComputePipelineId, ()>,
    pub(crate) query_sets: Pool<QuerySetId, u32>,
    surfaces: Pool<SurfaceId, NullSurface>,
}

impl State {
    fn remove_views_of(&mut self, texture: TextureId) {
        let views: Vec<TextureViewId> = self
            .views
            .iter()
            .filter(|(_, view)| view.texture == texture)
            .map(|(id, _)| id)
            .collect();
        for view in views {
            self.views.remove(view);
        }
    }
}

#[derive(Debug)]
pub struct NullDevice {
    caps: Caps,
    state: Mutex<State>,
    lost: AtomicBool,
}

impl NullDevice {
    pub fn new() -> Self {
        Self::with_features(Profile::High.features() | Features::TIMESTAMP_QUERY)
    }

    pub fn with_features(features: Features) -> Self {
        let formats = Format::ALL
            .iter()
            .map(|format| (*format, all_format_caps(*format)))
            .collect();
        let caps = Caps {
            backend: Backend::Null,
            adapter: AdapterInfo {
                name: "pathon null device".into(),
                driver: String::new(),
                device_type: DeviceType::Cpu,
            },
            features,
            limits: limits_for(features),
            formats,
            timestamp_period_ns: 1.0,
        };
        Self {
            caps,
            state: Mutex::new(State::default()),
            lost: AtomicBool::new(false),
        }
    }

    pub fn simulate_device_loss(&self) {
        self.lost.store(true, Ordering::SeqCst);
    }

    fn validate_binding_arrays(&self, desc: &BindGroupLayoutDesc) -> Result<(), RhiError> {
        let invalid = |message: String| RhiError::validation("R0107", format!("layout '{}': {message}", desc.label));
        let mut has_array = false;
        for entry in desc.entries.iter().filter(|entry| entry.count.is_some()) {
            has_array = true;
            if !self.caps.has(Features::TEXTURE_BINDING_ARRAY) {
                return Err(invalid("binding arrays need Features::TEXTURE_BINDING_ARRAY".into()));
            }
            let limit = match entry.ty {
                BindingType::SampledTexture { .. } | BindingType::StorageTexture { .. } => {
                    self.caps.limits.max_binding_array_elements_per_shader_stage
                }
                BindingType::Sampler(_) => self.caps.limits.max_binding_array_sampler_elements_per_shader_stage,
                _ => {
                    return Err(invalid(format!(
                        "binding {} is an array of buffers, which is unsupported",
                        entry.binding
                    )));
                }
            };
            let count = entry.count.map_or(0, std::num::NonZeroU32::get);
            if count > limit {
                return Err(invalid(format!(
                    "binding {} has {count} elements, the limit is {limit}",
                    entry.binding
                )));
            }
        }
        let has_uniform = desc.entries.iter().any(|entry| entry.ty == BindingType::UniformBuffer);
        if has_array && has_uniform {
            return Err(invalid(
                "a group with a binding array cannot contain a uniform buffer".into(),
            ));
        }
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("null device state is never poisoned: validation does not panic")
    }

    fn ensure_alive(&self) -> Result<(), RhiError> {
        if self.is_lost() {
            Err(RhiError::DeviceLost)
        } else {
            Ok(())
        }
    }
}

impl Default for NullDevice {
    fn default() -> Self {
        Self::new()
    }
}

fn limits_for(features: Features) -> Limits {
    let mut limits = Limits::WEBGPU_DEFAULT;
    if features.contains(Features::TEXTURE_BINDING_ARRAY) {
        limits.max_binding_array_elements_per_shader_stage = 1 << 20;
        limits.max_binding_array_sampler_elements_per_shader_stage = 2048;
    }
    limits
}

fn all_format_caps(format: Format) -> FormatCaps {
    if format.compression().is_some() {
        return FormatCaps::SAMPLED | FormatCaps::FILTERABLE;
    }
    let all = FormatCaps::all();
    if format.kind() == FormatKind::Color {
        all
    } else {
        all.difference(FormatCaps::STORAGE | FormatCaps::BLENDABLE)
    }
}

fn texture_use(ty: BindingType, view: TextureViewId) -> Option<BindingUse> {
    match ty {
        BindingType::SampledTexture { .. } => Some(BindingUse::Texture {
            view,
            access: Access::SAMPLED,
            reads: true,
        }),
        BindingType::StorageTexture { access, .. } => {
            let (needed, reads) = match access {
                StorageAccess::ReadOnly => (Access::STORAGE_READ, true),
                StorageAccess::WriteOnly => (Access::STORAGE_WRITE, false),
                StorageAccess::ReadWrite => (Access::STORAGE_READ | Access::STORAGE_WRITE, true),
            };
            Some(BindingUse::Texture {
                view,
                access: needed,
                reads,
            })
        }
        _ => None,
    }
}

fn binding_uses(layout_entry: &BindGroupLayoutEntry, resource: &BindingResource) -> Option<Vec<BindingUse>> {
    let ty = layout_entry.ty;
    match resource {
        BindingResource::TextureView(view) if layout_entry.count.is_none() => texture_use(ty, *view).map(|u| vec![u]),
        BindingResource::Buffer { .. }
            if layout_entry.count.is_none()
                && matches!(ty, BindingType::UniformBuffer | BindingType::StorageBuffer { .. }) =>
        {
            Some(vec![BindingUse::Buffer])
        }
        BindingResource::Sampler(_) if layout_entry.count.is_none() && matches!(ty, BindingType::Sampler(_)) => {
            Some(vec![BindingUse::Sampler])
        }
        BindingResource::TextureViewArray(views) if layout_entry.count.is_some() => {
            views.iter().map(|view| texture_use(ty, *view)).collect()
        }
        BindingResource::SamplerArray(samplers) if layout_entry.count.is_some() => {
            if matches!(ty, BindingType::Sampler(_)) {
                Some(samplers.iter().map(|_| BindingUse::Sampler).collect())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn resource_len(resource: &BindingResource) -> usize {
    match resource {
        BindingResource::TextureViewArray(views) => views.len(),
        BindingResource::SamplerArray(samplers) => samplers.len(),
        _ => 1,
    }
}

impl DeviceApi for NullDevice {
    fn caps(&self) -> &Caps {
        &self.caps
    }

    fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    fn create_buffer(&self, desc: &BufferDesc) -> Result<BufferId, RhiError> {
        self.ensure_alive()?;
        let size = usize::try_from(desc.size).map_err(|_| RhiError::OutOfMemory)?;
        Ok(self.lock().buffers.insert(NullBuffer {
            desc: *desc,
            data: vec![0; size],
        }))
    }

    fn write_buffer(&self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), RhiError> {
        let mut state = self.lock();
        let target = state.buffers.get_mut(buffer).ok_or(RhiError::InvalidHandle("buffer"))?;
        let start = usize::try_from(offset).map_err(|_| RhiError::OutOfMemory)?;
        let end = start + data.len();
        let Some(slice) = target.data.get_mut(start..end) else {
            return Err(RhiError::validation(
                "R0108",
                format!(
                    "write of {} bytes at {offset} overflows buffer '{}'",
                    data.len(),
                    target.desc.label
                ),
            ));
        };
        slice.copy_from_slice(data);
        Ok(())
    }

    fn create_texture(&self, desc: &TextureDesc) -> Result<TextureId, RhiError> {
        self.ensure_alive()?;
        if desc.mip_level_count == 0 || desc.mip_level_count > desc.size.max_mip_count() {
            return Err(RhiError::validation(
                "R0109",
                format!("texture '{}' has an invalid mip count", desc.label),
            ));
        }
        Ok(self.lock().textures.insert(NullTexture::new(*desc)))
    }

    fn texture_desc(&self, texture: TextureId) -> Result<TextureDesc, RhiError> {
        self.lock()
            .textures
            .get(texture)
            .map(|texture| texture.desc)
            .ok_or(RhiError::InvalidHandle("texture"))
    }

    fn write_texture(
        &self,
        dst: &TextureCopy,
        _data: &[u8],
        _layout: TexelLayout,
        size: Extent3d,
    ) -> Result<(), RhiError> {
        let mut state = self.lock();
        let texture = state
            .textures
            .get_mut(dst.texture)
            .ok_or(RhiError::InvalidHandle("texture"))?;
        let layers = if texture.desc.dimension == TextureDimension::D3 {
            1
        } else {
            size.depth_or_layers
        };
        // A 3D texture has one subresource per mip; `origin.z` is a depth slice inside it, not a layer.
        let base_layer = if texture.desc.dimension == TextureDimension::D3 {
            0
        } else {
            dst.origin.z
        };
        let range = SubresourceRange {
            base_mip: dst.mip,
            mip_count: 1,
            base_layer,
            layer_count: layers,
        };
        for (mip, layer) in texture.subresource_list(range) {
            texture.subresource_mut(mip, layer).initialized = true;
        }
        Ok(())
    }

    fn create_texture_view(&self, texture: TextureId, desc: &TextureViewDesc) -> Result<TextureViewId, RhiError> {
        let mut state = self.lock();
        let target = state.textures.get(texture).ok_or(RhiError::InvalidHandle("texture"))?;
        let range = SubresourceRange {
            base_mip: desc.base_mip,
            mip_count: desc.mip_count.unwrap_or(u32::MAX),
            base_layer: desc.base_layer,
            layer_count: desc.layer_count.unwrap_or(u32::MAX),
        }
        .resolve(target.desc.mip_level_count, target.desc.array_layers());
        if range.mip_count == 0 || range.layer_count == 0 {
            return Err(RhiError::validation(
                "R0109",
                format!("view of '{}' selects no subresources", target.desc.label),
            ));
        }
        Ok(state.views.insert(NullView { texture, range }))
    }

    fn create_sampler(&self, _desc: &SamplerDesc) -> Result<SamplerId, RhiError> {
        Ok(self.lock().samplers.insert(()))
    }

    fn create_shader_module(&self, desc: &ShaderModuleDesc<'_>) -> Result<ShaderModuleId, RhiError> {
        if desc.wgsl.trim().is_empty() {
            return Err(RhiError::validation(
                "R0110",
                format!("shader module '{}' is empty", desc.label),
            ));
        }
        Ok(self.lock().modules.insert(()))
    }

    fn create_bind_group_layout(&self, desc: &BindGroupLayoutDesc) -> Result<BindGroupLayoutId, RhiError> {
        self.validate_binding_arrays(desc)?;
        Ok(self.lock().layouts.insert(desc.clone()))
    }

    fn create_bind_group(&self, desc: &BindGroupDesc) -> Result<BindGroupId, RhiError> {
        let mut state = self.lock();
        let layout = state
            .layouts
            .get(desc.layout)
            .ok_or(RhiError::InvalidHandle("bind group layout"))?;
        let mut uses = Vec::with_capacity(layout.entries.len());
        for layout_entry in &layout.entries {
            let entry = desc
                .entries
                .iter()
                .find(|entry| entry.binding == layout_entry.binding)
                .ok_or_else(|| {
                    RhiError::validation(
                        "R0107",
                        format!("bind group '{}' misses binding {}", desc.label, layout_entry.binding),
                    )
                })?;
            let entry_uses = binding_uses(layout_entry, &entry.resource).ok_or_else(|| {
                RhiError::validation(
                    "R0107",
                    format!(
                        "bind group '{}' binding {} has the wrong resource kind",
                        desc.label, entry.binding
                    ),
                )
            })?;
            if let Some(count) = layout_entry.count {
                let len = resource_len(&entry.resource);
                let allowed = if self.caps.has(Features::PARTIALLY_BOUND_BINDING_ARRAY) {
                    len <= count.get() as usize
                } else {
                    len == count.get() as usize
                };
                if !allowed {
                    return Err(RhiError::validation(
                        "R0107",
                        format!(
                            "bind group '{}' binding {} has {len} array elements for a layout count of {count}",
                            desc.label, entry.binding
                        ),
                    ));
                }
            }
            uses.extend(entry_uses);
        }
        for binding_use in &uses {
            check_binding_usage(&state, desc.label, *binding_use)?;
        }
        Ok(state.bind_groups.insert(uses))
    }

    fn create_render_pipeline(&self, desc: &RenderPipelineDesc) -> Result<RenderPipelineId, RhiError> {
        let mut state = self.lock();
        for layout in &desc.bind_group_layouts {
            if !state.layouts.contains(*layout) {
                return Err(RhiError::InvalidHandle("bind group layout"));
            }
        }
        Ok(state.render_pipelines.insert(()))
    }

    fn create_compute_pipeline(&self, desc: &ComputePipelineDesc) -> Result<ComputePipelineId, RhiError> {
        let mut state = self.lock();
        for layout in &desc.bind_group_layouts {
            if !state.layouts.contains(*layout) {
                return Err(RhiError::InvalidHandle("bind group layout"));
            }
        }
        Ok(state.compute_pipelines.insert(()))
    }

    fn create_query_set(&self, desc: &QuerySetDesc) -> Result<QuerySetId, RhiError> {
        Ok(self.lock().query_sets.insert(desc.count))
    }

    fn destroy(&self, resource: Resource) {
        let mut state = self.lock();
        match resource {
            Resource::Buffer(id) => drop(state.buffers.remove(id)),
            Resource::Texture(id) => drop(state.textures.remove(id)),
            Resource::TextureView(id) => drop(state.views.remove(id)),
            Resource::Sampler(id) => drop(state.samplers.remove(id)),
            Resource::ShaderModule(id) => drop(state.modules.remove(id)),
            Resource::BindGroupLayout(id) => drop(state.layouts.remove(id)),
            Resource::BindGroup(id) => drop(state.bind_groups.remove(id)),
            Resource::RenderPipeline(id) => drop(state.render_pipelines.remove(id)),
            Resource::ComputePipeline(id) => drop(state.compute_pipelines.remove(id)),
            Resource::QuerySet(id) => drop(state.query_sets.remove(id)),
            _ => unreachable!("null device cannot destroy {resource:?}"),
        }
    }

    fn create_surface(&self, _target: &SurfaceTarget) -> Result<SurfaceId, RhiError> {
        Ok(self.lock().surfaces.insert(NullSurface {
            config: None,
            frame: None,
        }))
    }

    fn configure_surface(&self, surface: SurfaceId, config: &SurfaceConfig) -> Result<Format, RhiError> {
        if config.size.is_empty() {
            return Err(RhiError::SurfaceUnavailable);
        }
        let mut state = self.lock();
        let target = state
            .surfaces
            .get_mut(surface)
            .ok_or(RhiError::InvalidHandle("surface"))?;
        target.config = Some(*config);
        Ok(config.format.unwrap_or(Format::Bgra8UnormSrgb))
    }

    fn acquire_surface(&self, surface: SurfaceId) -> Result<SurfaceFrame, RhiError> {
        self.ensure_alive()?;
        let mut state = self.lock();
        let target = state.surfaces.get(surface).ok_or(RhiError::InvalidHandle("surface"))?;
        let config = target
            .config
            .ok_or_else(|| RhiError::validation("R0111", "surface acquired before configure"))?;
        if target.frame.is_some() {
            return Err(RhiError::validation("R0111", "surface acquired twice without present"));
        }
        let format = config.format.unwrap_or(Format::Bgra8UnormSrgb);
        let desc = TextureDesc::new_2d(
            "surface frame",
            config.size.width,
            config.size.height,
            format,
            TextureUsage::RENDER_TARGET | TextureUsage::COPY_DST,
        );
        let texture = state.textures.insert(NullTexture::new(desc));
        let view = state.views.insert(NullView {
            texture,
            range: SubresourceRange::ALL.resolve(1, 1),
        });
        if let Some(target) = state.surfaces.get_mut(surface) {
            target.frame = Some(texture);
        }
        Ok(SurfaceFrame {
            texture,
            view,
            format,
            size: config.size,
        })
    }

    fn present_surface(&self, surface: SurfaceId) -> Result<(), RhiError> {
        let mut state = self.lock();
        let target = state
            .surfaces
            .get_mut(surface)
            .ok_or(RhiError::InvalidHandle("surface"))?;
        let texture = target
            .frame
            .take()
            .ok_or_else(|| RhiError::validation("R0111", "present without acquire"))?;
        let frame = state
            .textures
            .remove(texture)
            .ok_or(RhiError::InvalidHandle("texture"))?;
        let presentable = frame
            .subresources
            .iter()
            .all(|sub| sub.access == Access::PRESENT && sub.initialized);
        state.remove_views_of(texture);
        if presentable {
            Ok(())
        } else {
            Err(RhiError::validation(
                "R0106",
                "surface frame presented without being written and moved to PRESENT",
            ))
        }
    }

    fn discard_surface(&self, surface: SurfaceId) {
        let mut state = self.lock();
        if let Some(texture) = state.surfaces.get_mut(surface).and_then(|target| target.frame.take()) {
            state.textures.remove(texture);
            state.remove_views_of(texture);
        }
    }

    fn destroy_surface(&self, surface: SurfaceId) {
        self.lock().surfaces.remove(surface);
    }

    fn submit(&self, commands: CommandList) -> Result<(), RhiError> {
        self.ensure_alive()?;
        let mut state = self.lock();
        Validator::new(&mut state).run(commands)
    }

    fn read_buffer(&self, buffer: BufferId) -> Result<Vec<u8>, RhiError> {
        let state = self.lock();
        let target = state.buffers.get(buffer).ok_or(RhiError::InvalidHandle("buffer"))?;
        if !target.desc.usage.contains(BufferUsage::MAP_READ) {
            return Err(RhiError::validation(
                "R0105",
                format!("buffer '{}' read back without MAP_READ", target.desc.label),
            ));
        }
        Ok(target.data.clone())
    }

    fn wait_idle(&self) -> Result<(), RhiError> {
        self.ensure_alive()
    }
}

fn check_binding_usage(state: &State, label: &str, binding_use: BindingUse) -> Result<(), RhiError> {
    let BindingUse::Texture { view, access, .. } = binding_use else {
        return Ok(());
    };
    let view = state.views.get(view).ok_or(RhiError::InvalidHandle("texture view"))?;
    let texture = state
        .textures
        .get(view.texture)
        .ok_or(RhiError::InvalidHandle("texture"))?;
    let needed = if access.intersects(Access::STORAGE_READ | Access::STORAGE_WRITE) {
        TextureUsage::STORAGE
    } else {
        TextureUsage::SAMPLED
    };
    if texture.desc.usage.contains(needed) {
        Ok(())
    } else {
        Err(RhiError::validation(
            "R0105",
            format!(
                "bind group '{label}' binds texture '{}' without {needed:?} usage",
                texture.desc.label
            ),
        ))
    }
}
