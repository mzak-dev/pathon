use std::borrow::Cow;
use std::fmt;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use pathon_rhi_types::{
    AdapterInfo, Backend, BindGroupDesc, BindGroupId, BindGroupLayoutDesc, BindGroupLayoutId, BindingResource,
    BufferDesc, BufferId, Caps, CommandList, Compression, ComputePipelineDesc, ComputePipelineId, DeviceApi,
    DeviceRequest, Extent3d, Features, Format, FormatCaps, Pool, PowerPreference, QuerySetDesc, QuerySetId,
    RenderPipelineDesc, RenderPipelineId, Resource, RhiError, SamplerDesc, SamplerId, ShaderModuleDesc, ShaderModuleId,
    SurfaceConfig, SurfaceFrame, SurfaceId, SurfaceTarget, TexelLayout, TextureCopy, TextureDesc, TextureId,
    TextureViewDesc, TextureViewId, WindowSource,
};
use raw_window_handle::{DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle};

use crate::interop::DeviceHook;
use crate::{convert, encode, poll};

#[derive(Debug)]
pub(crate) struct TextureEntry {
    pub(crate) texture: wgpu::Texture,
    pub(crate) desc: TextureDesc,
}

struct AcquiredFrame {
    surface_texture: wgpu::SurfaceTexture,
    texture: TextureId,
    view: TextureViewId,
}

struct SurfaceEntry {
    surface: wgpu::Surface<'static>,
    window: Arc<dyn WindowSource>,
    claimed: bool,
    config: Option<wgpu::SurfaceConfiguration>,
    frame: Option<AcquiredFrame>,
}

#[derive(Debug, Default)]
pub(crate) struct Objects {
    pub(crate) buffers: Pool<BufferId, wgpu::Buffer>,
    pub(crate) textures: Pool<TextureId, TextureEntry>,
    pub(crate) views: Pool<TextureViewId, wgpu::TextureView>,
    pub(crate) samplers: Pool<SamplerId, wgpu::Sampler>,
    pub(crate) modules: Pool<ShaderModuleId, wgpu::ShaderModule>,
    pub(crate) layouts: Pool<BindGroupLayoutId, wgpu::BindGroupLayout>,
    pub(crate) bind_groups: Pool<BindGroupId, wgpu::BindGroup>,
    pub(crate) render_pipelines: Pool<RenderPipelineId, wgpu::RenderPipeline>,
    pub(crate) compute_pipelines: Pool<ComputePipelineId, wgpu::ComputePipeline>,
    pub(crate) query_sets: Pool<QuerySetId, wgpu::QuerySet>,
    surfaces: Pool<SurfaceId, SurfaceEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChosenSurfaceFormat {
    configured: wgpu::TextureFormat,
    view: wgpu::TextureFormat,
}

impl ChosenSurfaceFormat {
    fn view_formats(self) -> Vec<wgpu::TextureFormat> {
        if self.view == self.configured {
            Vec::new()
        } else {
            vec![self.view]
        }
    }
}

// WebGPU surfaces offer only non-sRGB formats; rendering through an sRGB view of them keeps colours equal to native.
fn choose_surface_format(
    wanted: Option<wgpu::TextureFormat>,
    available: &[wgpu::TextureFormat],
) -> Option<ChosenSurfaceFormat> {
    let understood = |format: &wgpu::TextureFormat| convert::format_from_wgpu(*format).is_some();
    if let Some(wanted) = wanted.filter(|format| available.contains(format)) {
        return Some(ChosenSurfaceFormat {
            configured: wanted,
            view: wanted,
        });
    }
    let candidates: Vec<wgpu::TextureFormat> = available.iter().copied().filter(understood).collect();
    if let Some(srgb) = candidates.iter().copied().find(wgpu::TextureFormat::is_srgb) {
        return Some(ChosenSurfaceFormat {
            configured: srgb,
            view: srgb,
        });
    }
    let configured = candidates.first().copied()?;
    let view = Some(configured.add_srgb_suffix())
        .filter(understood)
        .unwrap_or(configured);
    Some(ChosenSurfaceFormat { configured, view })
}

// Lets wgpu own a surface for as long as pathon keeps the window alive.
struct SharedWindow(Arc<dyn WindowSource>);

impl HasWindowHandle for SharedWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.0.window_handle()
    }
}

impl HasDisplayHandle for SharedWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.0.display_handle()
    }
}

fn create_wgpu_surface(
    instance: &wgpu::Instance,
    window: &Arc<dyn WindowSource>,
) -> Result<wgpu::Surface<'static>, RhiError> {
    instance
        .create_surface(SharedWindow(Arc::clone(window)))
        .map_err(|error| RhiError::Backend(format!("surface creation failed: {error}")))
}

pub struct WgpuDevice {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    caps: Caps,
    objects: Mutex<Objects>,
    lost: Arc<AtomicBool>,
}

impl fmt::Debug for WgpuDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WgpuDevice")
            .field("caps", &self.caps)
            .finish_non_exhaustive()
    }
}

impl WgpuDevice {
    pub async fn new(request: &DeviceRequest) -> Result<Self, RhiError> {
        if request.backend == Some(Backend::Null) {
            return Err(RhiError::Unsupported("the wgpu backend cannot create a null device"));
        }
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = convert::backends_to_wgpu(request.backend);
        instance_desc.flags = wgpu::InstanceFlags::from_build_config();
        let instance = wgpu::Instance::new(instance_desc);

        let compatible = match &request.compatible_surface {
            Some(target) => Some((
                Arc::clone(target.window()),
                create_wgpu_surface(&instance, target.window())?,
            )),
            None => None,
        };
        let power_preference = match request.power {
            PowerPreference::HighPerformance => wgpu::PowerPreference::HighPerformance,
            PowerPreference::LowPower => wgpu::PowerPreference::LowPower,
        };
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
                compatible_surface: compatible.as_ref().map(|(_, surface)| surface),
            })
            .await
            .map_err(|error| RhiError::NoAdapter(error.to_string()))?;

        let adapter_features = convert::features_from_wgpu(adapter.features());
        let missing = request.required_features.difference(adapter_features);
        if !missing.is_empty() {
            return Err(RhiError::MissingFeatures(missing));
        }
        let enabled = (request.required_features | (request.optional_features & adapter_features))
            .difference(convert::EXPERIMENTAL);
        let mut descriptor = wgpu::DeviceDescriptor {
            label: Some(request.label),
            required_features: convert::features_to_wgpu(enabled),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        };
        for extension in &request.extensions {
            if let Some(DeviceHook(hook)) = extension.downcast_ref::<DeviceHook>() {
                hook(&mut descriptor);
            }
        }
        let (device, queue) = adapter
            .request_device(&descriptor)
            .await
            .map_err(|error| RhiError::DeviceCreation(error.to_string()))?;

        let this = Self::from_parts(instance, adapter, device, queue);
        this.install_handlers();
        if let Some((window, surface)) = compatible {
            this.lock().surfaces.insert(SurfaceEntry {
                surface,
                window,
                claimed: false,
                config: None,
                frame: None,
            });
        }
        Ok(this)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn new_blocking(request: &DeviceRequest) -> Result<Self, RhiError> {
        poll::ready(Self::new(request)).unwrap_or(Err(RhiError::Unsupported("native wgpu future did not resolve")))
    }

    pub(crate) fn from_parts(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Self {
        let caps = build_caps(&adapter, &device, &queue);
        Self {
            instance,
            adapter,
            device,
            queue,
            caps,
            objects: Mutex::new(Objects::default()),
            lost: Arc::new(AtomicBool::new(false)),
        }
    }

    // Only on devices pathon creates: wgpu keeps one handler of each kind, and an adopted device keeps the host's
    // (the host reports loss through `Renderer::reset`, ADR-0005).
    fn install_handlers(&self) {
        let lost = Arc::clone(&self.lost);
        self.device.set_device_lost_callback(move |reason, message| {
            log::warn!("wgpu device lost ({reason:?}): {message}");
            lost.store(true, Ordering::SeqCst);
        });
        self.device
            .on_uncaptured_error(Arc::new(|error: wgpu::Error| log::error!("wgpu: {error}")));
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Objects> {
        self.objects
            .lock()
            .expect("wgpu object pools are never poisoned: no code panics while holding the lock")
    }

    pub(crate) fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    pub(crate) fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    pub(crate) fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub(crate) fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    // Validation errors become `Err` instead of reaching the uncaptured-error handler. On the web the scope resolves
    // asynchronously, so errors there are only logged.
    fn scoped<T>(&self, create: impl FnOnce() -> T) -> Result<T, RhiError> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let value = create();
        match poll::ready(scope.pop()) {
            Some(Some(error)) => Err(RhiError::Backend(error.to_string())),
            _ => Ok(value),
        }
    }

    fn texture_entry<R>(&self, texture: TextureId, read: impl FnOnce(&TextureEntry) -> R) -> Result<R, RhiError> {
        self.lock()
            .textures
            .get(texture)
            .map(read)
            .ok_or(RhiError::InvalidHandle("texture"))
    }
}

fn format_enabled(format: Format, features: Features) -> bool {
    match format.compression() {
        Some(Compression::Bc) => features.contains(Features::TEXTURE_COMPRESSION_BC),
        Some(Compression::Etc2) => features.contains(Features::TEXTURE_COMPRESSION_ETC2),
        Some(Compression::Astc) => features.contains(Features::TEXTURE_COMPRESSION_ASTC),
        None => match format {
            Format::R16Unorm | Format::Rgba16Unorm => features.contains(Features::TEXTURE_FORMAT_16BIT_NORM),
            Format::Depth32FloatStencil8 => features.contains(Features::DEPTH32FLOAT_STENCIL8),
            _ => true,
        },
    }
}

fn build_caps(adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Caps {
    let info = adapter.get_info();
    let features = convert::features_from_wgpu(device.features());
    let formats = Format::ALL
        .iter()
        .map(|format| {
            let caps = match convert::format_to_wgpu(*format) {
                Ok(wgpu_format) if format_enabled(*format, features) => {
                    convert::format_caps_from_wgpu(adapter.get_texture_format_features(wgpu_format))
                }
                _ => FormatCaps::empty(),
            };
            (*format, caps)
        })
        .collect();
    Caps {
        backend: convert::backend_from_wgpu(info.backend),
        adapter: AdapterInfo {
            name: info.name,
            driver: format!("{} {}", info.driver, info.driver_info).trim().to_owned(),
            device_type: convert::device_type_from_wgpu(info.device_type),
        },
        features,
        limits: convert::limits_from_wgpu(&device.limits()),
        formats,
        timestamp_period_ns: queue.get_timestamp_period(),
    }
}

fn extent_to_wgpu(size: Extent3d) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: size.width,
        height: size.height,
        depth_or_array_layers: size.depth_or_layers,
    }
}

impl DeviceApi for WgpuDevice {
    fn caps(&self) -> &Caps {
        &self.caps
    }

    fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    fn create_buffer(&self, desc: &BufferDesc) -> Result<BufferId, RhiError> {
        let buffer = self.scoped(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(desc.label),
                size: desc.size,
                usage: convert::buffer_usage_to_wgpu(desc.usage),
                mapped_at_creation: false,
            })
        })?;
        Ok(self.lock().buffers.insert(buffer))
    }

    fn write_buffer(&self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), RhiError> {
        let objects = self.lock();
        let target = objects.buffers.get(buffer).ok_or(RhiError::InvalidHandle("buffer"))?;
        self.queue.write_buffer(target, offset, data);
        Ok(())
    }

    fn create_texture(&self, desc: &TextureDesc) -> Result<TextureId, RhiError> {
        let format = convert::format_to_wgpu(desc.format)?;
        let texture = self.scoped(|| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(desc.label),
                size: extent_to_wgpu(desc.size),
                mip_level_count: desc.mip_level_count,
                sample_count: desc.sample_count,
                dimension: convert::texture_dimension_to_wgpu(desc.dimension),
                format,
                usage: convert::texture_usage_to_wgpu(desc.usage),
                view_formats: &[],
            })
        })?;
        Ok(self.lock().textures.insert(TextureEntry { texture, desc: *desc }))
    }

    fn texture_desc(&self, texture: TextureId) -> Result<TextureDesc, RhiError> {
        self.texture_entry(texture, |entry| entry.desc)
    }

    fn write_texture(
        &self,
        dst: &TextureCopy,
        data: &[u8],
        layout: TexelLayout,
        size: Extent3d,
    ) -> Result<(), RhiError> {
        let objects = self.lock();
        let entry = objects
            .textures
            .get(dst.texture)
            .ok_or(RhiError::InvalidHandle("texture"))?;
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &entry.texture,
                mip_level: dst.mip,
                origin: wgpu::Origin3d {
                    x: dst.origin.x,
                    y: dst.origin.y,
                    z: dst.origin.z,
                },
                aspect: convert::aspect_to_wgpu(dst.aspect),
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: layout.offset,
                bytes_per_row: Some(layout.bytes_per_row),
                rows_per_image: Some(layout.rows_per_image),
            },
            extent_to_wgpu(size),
        );
        Ok(())
    }

    fn create_texture_view(&self, texture: TextureId, desc: &TextureViewDesc) -> Result<TextureViewId, RhiError> {
        let view_format = desc.format.map(convert::format_to_wgpu).transpose()?;
        let view = self.texture_entry(texture, |entry| {
            self.scoped(|| {
                entry.texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(entry.desc.label),
                    format: view_format,
                    dimension: desc.dimension.map(convert::view_dimension_to_wgpu),
                    aspect: convert::aspect_to_wgpu(desc.aspect),
                    base_mip_level: desc.base_mip,
                    mip_level_count: desc.mip_count,
                    base_array_layer: desc.base_layer,
                    array_layer_count: desc.layer_count,
                    ..Default::default()
                })
            })
        })??;
        Ok(self.lock().views.insert(view))
    }

    fn create_sampler(&self, desc: &SamplerDesc) -> Result<SamplerId, RhiError> {
        let address_mode = convert::address_mode_to_wgpu(desc.address_mode);
        let sampler = self.scoped(|| {
            self.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(desc.label),
                address_mode_u: address_mode,
                address_mode_v: address_mode,
                address_mode_w: address_mode,
                mag_filter: convert::filter_to_wgpu(desc.mag_filter),
                min_filter: convert::filter_to_wgpu(desc.min_filter),
                mipmap_filter: convert::mipmap_filter_to_wgpu(desc.mipmap_filter),
                lod_min_clamp: desc.lod_min_clamp,
                lod_max_clamp: desc.lod_max_clamp,
                compare: desc.compare.map(convert::compare_to_wgpu),
                anisotropy_clamp: desc.anisotropy_clamp,
                border_color: None,
            })
        })?;
        Ok(self.lock().samplers.insert(sampler))
    }

    fn create_shader_module(&self, desc: &ShaderModuleDesc<'_>) -> Result<ShaderModuleId, RhiError> {
        let module = self.scoped(|| {
            self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(desc.label),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(desc.wgsl)),
            })
        })?;
        Ok(self.lock().modules.insert(module))
    }

    fn create_bind_group_layout(&self, desc: &BindGroupLayoutDesc) -> Result<BindGroupLayoutId, RhiError> {
        let entries: Vec<wgpu::BindGroupLayoutEntry> = desc
            .entries
            .iter()
            .map(|entry| {
                Ok(wgpu::BindGroupLayoutEntry {
                    binding: entry.binding,
                    visibility: convert::shader_stages_to_wgpu(entry.visibility),
                    ty: convert::binding_type_to_wgpu(entry.ty)?,
                    count: None,
                })
            })
            .collect::<Result<_, RhiError>>()?;
        let layout = self.scoped(|| {
            self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(desc.label),
                entries: &entries,
            })
        })?;
        Ok(self.lock().layouts.insert(layout))
    }

    fn create_bind_group(&self, desc: &BindGroupDesc) -> Result<BindGroupId, RhiError> {
        let mut objects = self.lock();
        let layout = objects
            .layouts
            .get(desc.layout)
            .ok_or(RhiError::InvalidHandle("bind group layout"))?;
        let mut entries = Vec::with_capacity(desc.entries.len());
        for entry in &desc.entries {
            let resource = match entry.resource {
                BindingResource::Buffer { buffer, offset, size } => {
                    let buffer = objects.buffers.get(buffer).ok_or(RhiError::InvalidHandle("buffer"))?;
                    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer,
                        offset,
                        size: size.and_then(NonZeroU64::new),
                    })
                }
                BindingResource::TextureView(view) => wgpu::BindingResource::TextureView(
                    objects.views.get(view).ok_or(RhiError::InvalidHandle("texture view"))?,
                ),
                BindingResource::Sampler(sampler) => wgpu::BindingResource::Sampler(
                    objects
                        .samplers
                        .get(sampler)
                        .ok_or(RhiError::InvalidHandle("sampler"))?,
                ),
            };
            entries.push(wgpu::BindGroupEntry {
                binding: entry.binding,
                resource,
            });
        }
        let group = self.scoped(|| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(desc.label),
                layout,
                entries: &entries,
            })
        })?;
        drop(entries);
        Ok(objects.bind_groups.insert(group))
    }

    fn create_render_pipeline(&self, desc: &RenderPipelineDesc) -> Result<RenderPipelineId, RhiError> {
        let mut objects = self.lock();
        let layout = pipeline_layout(&self.device, &objects, desc.label, &desc.bind_group_layouts)?;
        let vertex_module = objects
            .modules
            .get(desc.vertex.module)
            .ok_or(RhiError::InvalidHandle("shader module"))?;
        let attributes = desc
            .vertex_buffers
            .iter()
            .map(|buffer| {
                buffer
                    .attributes
                    .iter()
                    .map(|attribute| {
                        Ok(wgpu::VertexAttribute {
                            format: convert::vertex_format_to_wgpu(attribute.format)?,
                            offset: attribute.offset,
                            shader_location: attribute.location,
                        })
                    })
                    .collect::<Result<Vec<_>, RhiError>>()
            })
            .collect::<Result<Vec<_>, RhiError>>()?;
        let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = desc
            .vertex_buffers
            .iter()
            .zip(&attributes)
            .map(|(buffer, attributes)| {
                Some(wgpu::VertexBufferLayout {
                    array_stride: buffer.stride,
                    step_mode: convert::step_mode_to_wgpu(buffer.step_mode),
                    attributes,
                })
            })
            .collect();
        let targets: Vec<Option<wgpu::ColorTargetState>> = desc
            .color_targets
            .iter()
            .map(|target| {
                Ok(Some(wgpu::ColorTargetState {
                    format: convert::format_to_wgpu(target.format)?,
                    blend: target.blend.map(convert::blend_to_wgpu),
                    write_mask: convert::color_writes_to_wgpu(target.write_mask),
                }))
            })
            .collect::<Result<_, RhiError>>()?;
        let fragment = match desc.fragment {
            Some(stage) => Some(wgpu::FragmentState {
                module: objects
                    .modules
                    .get(stage.module)
                    .ok_or(RhiError::InvalidHandle("shader module"))?,
                entry_point: Some(stage.entry_point),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &targets,
            }),
            None => None,
        };
        let depth_stencil = desc
            .depth
            .map(|depth| {
                Ok(wgpu::DepthStencilState {
                    format: convert::format_to_wgpu(depth.format)?,
                    depth_write_enabled: Some(depth.write_enabled),
                    depth_compare: Some(convert::compare_to_wgpu(depth.compare)),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                })
            })
            .transpose()?;
        let pipeline = self.scoped(|| {
            self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(desc.label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: vertex_module,
                    entry_point: Some(desc.vertex.entry_point),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: convert::topology_to_wgpu(desc.primitive.topology),
                    front_face: convert::front_face_to_wgpu(desc.primitive.front_face),
                    cull_mode: convert::cull_mode_to_wgpu(desc.primitive.cull_mode),
                    ..wgpu::PrimitiveState::default()
                },
                depth_stencil,
                multisample: wgpu::MultisampleState {
                    count: desc.sample_count,
                    ..wgpu::MultisampleState::default()
                },
                fragment,
                multiview_mask: None,
                cache: None,
            })
        })?;
        drop(buffers);
        Ok(objects.render_pipelines.insert(pipeline))
    }

    fn create_compute_pipeline(&self, desc: &ComputePipelineDesc) -> Result<ComputePipelineId, RhiError> {
        let mut objects = self.lock();
        let layout = pipeline_layout(&self.device, &objects, desc.label, &desc.bind_group_layouts)?;
        let module = objects
            .modules
            .get(desc.shader.module)
            .ok_or(RhiError::InvalidHandle("shader module"))?;
        let pipeline = self.scoped(|| {
            self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(desc.label),
                layout: Some(&layout),
                module,
                entry_point: Some(desc.shader.entry_point),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        })?;
        Ok(objects.compute_pipelines.insert(pipeline))
    }

    fn create_query_set(&self, desc: &QuerySetDesc) -> Result<QuerySetId, RhiError> {
        if !self.caps.has(Features::TIMESTAMP_QUERY) {
            return Err(RhiError::MissingFeatures(Features::TIMESTAMP_QUERY));
        }
        let query_set = self.scoped(|| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some(desc.label),
                ty: wgpu::QueryType::Timestamp,
                count: desc.count,
            })
        })?;
        Ok(self.lock().query_sets.insert(query_set))
    }

    fn destroy(&self, resource: Resource) {
        let mut objects = self.lock();
        match resource {
            Resource::Buffer(id) => drop(objects.buffers.remove(id)),
            Resource::Texture(id) => drop(objects.textures.remove(id)),
            Resource::TextureView(id) => drop(objects.views.remove(id)),
            Resource::Sampler(id) => drop(objects.samplers.remove(id)),
            Resource::ShaderModule(id) => drop(objects.modules.remove(id)),
            Resource::BindGroupLayout(id) => drop(objects.layouts.remove(id)),
            Resource::BindGroup(id) => drop(objects.bind_groups.remove(id)),
            Resource::RenderPipeline(id) => drop(objects.render_pipelines.remove(id)),
            Resource::ComputePipeline(id) => drop(objects.compute_pipelines.remove(id)),
            Resource::QuerySet(id) => drop(objects.query_sets.remove(id)),
            other => log::error!("wgpu device cannot destroy {other:?}; the resource is leaked"),
        }
    }

    fn create_surface(&self, target: &SurfaceTarget) -> Result<SurfaceId, RhiError> {
        let mut objects = self.lock();
        let unclaimed = objects
            .surfaces
            .iter()
            .find(|(_, entry)| !entry.claimed && Arc::ptr_eq(&entry.window, target.window()))
            .map(|(id, _)| id);
        if let Some(id) = unclaimed
            && let Some(entry) = objects.surfaces.get_mut(id)
        {
            entry.claimed = true;
            return Ok(id);
        }
        let surface = create_wgpu_surface(&self.instance, target.window())?;
        let window = Arc::clone(target.window());
        Ok(objects.surfaces.insert(SurfaceEntry {
            surface,
            window,
            claimed: true,
            config: None,
            frame: None,
        }))
    }

    fn configure_surface(&self, surface: SurfaceId, config: &SurfaceConfig) -> Result<Format, RhiError> {
        if config.size.is_empty() {
            return Err(RhiError::SurfaceUnavailable);
        }
        let mut objects = self.lock();
        let entry = objects
            .surfaces
            .get_mut(surface)
            .ok_or(RhiError::InvalidHandle("surface"))?;
        let capabilities = entry.surface.get_capabilities(&self.adapter);
        let wanted = config.format.and_then(|format| convert::format_to_wgpu(format).ok());
        let chosen = choose_surface_format(wanted, &capabilities.formats)
            .ok_or(RhiError::Unsupported("surface offers no format pathon understands"))?;
        let format = convert::format_from_wgpu(chosen.view).ok_or(RhiError::Unsupported("surface format"))?;
        let mut wgpu_config = entry
            .surface
            .get_default_config(&self.adapter, config.size.width, config.size.height)
            .ok_or(RhiError::Unsupported("surface is not supported by this adapter"))?;
        wgpu_config.format = chosen.configured;
        wgpu_config.view_formats = chosen.view_formats();
        wgpu_config.present_mode = convert::present_mode_to_wgpu(config.present_mode, &capabilities.present_modes);
        wgpu_config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT
            | (capabilities.usages & (wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC));
        wgpu_config.desired_maximum_frame_latency = 2;
        entry.surface.configure(&self.device, &wgpu_config);
        entry.config = Some(wgpu_config);
        Ok(format)
    }

    fn acquire_surface(&self, surface: SurfaceId) -> Result<SurfaceFrame, RhiError> {
        if self.is_lost() {
            return Err(RhiError::DeviceLost);
        }
        let mut guard = self.lock();
        let objects = &mut *guard;
        let entry = objects
            .surfaces
            .get_mut(surface)
            .ok_or(RhiError::InvalidHandle("surface"))?;
        if entry.frame.is_some() {
            return Err(RhiError::validation("R0111", "surface acquired twice without present"));
        }
        let config = entry
            .config
            .clone()
            .ok_or_else(|| RhiError::validation("R0111", "surface acquired before configure"))?;
        let surface_texture = match entry.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                entry.surface.configure(&self.device, &config);
                match entry.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(texture)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
                    wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                        return Err(RhiError::SurfaceUnavailable);
                    }
                    _ => return Err(RhiError::SurfaceLost),
                }
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Err(RhiError::SurfaceUnavailable);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(RhiError::Backend(
                    "surface texture acquisition failed validation".into(),
                ));
            }
        };
        let view_format = config.view_formats.first().copied().unwrap_or(config.format);
        let format = convert::format_from_wgpu(view_format).ok_or(RhiError::Unsupported("surface format"))?;
        let desc = TextureDesc::new_2d(
            "surface frame",
            config.width,
            config.height,
            format,
            convert::texture_usage_from_wgpu(config.usage),
        );
        let view = surface_texture.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(view_format),
            ..Default::default()
        });
        let texture = objects.textures.insert(TextureEntry {
            texture: surface_texture.texture.clone(),
            desc,
        });
        let view = objects.views.insert(view);
        entry.frame = Some(AcquiredFrame {
            surface_texture,
            texture,
            view,
        });
        Ok(SurfaceFrame {
            texture,
            view,
            format,
            size: pathon_rhi_types::PhysicalSize::new(config.width, config.height),
        })
    }

    fn present_surface(&self, surface: SurfaceId) -> Result<(), RhiError> {
        let mut guard = self.lock();
        let objects = &mut *guard;
        let entry = objects
            .surfaces
            .get_mut(surface)
            .ok_or(RhiError::InvalidHandle("surface"))?;
        let frame = entry
            .frame
            .take()
            .ok_or_else(|| RhiError::validation("R0111", "present without acquire"))?;
        objects.views.remove(frame.view);
        objects.textures.remove(frame.texture);
        drop(guard);
        self.queue.present(frame.surface_texture);
        Ok(())
    }

    // Dropping an unpresented `SurfaceTexture` discards it.
    fn discard_surface(&self, surface: SurfaceId) {
        let mut guard = self.lock();
        let objects = &mut *guard;
        if let Some(frame) = objects.surfaces.get_mut(surface).and_then(|entry| entry.frame.take()) {
            objects.views.remove(frame.view);
            objects.textures.remove(frame.texture);
        }
    }

    fn destroy_surface(&self, surface: SurfaceId) {
        let mut guard = self.lock();
        let objects = &mut *guard;
        if let Some(entry) = objects.surfaces.remove(surface)
            && let Some(frame) = entry.frame
        {
            objects.views.remove(frame.view);
            objects.textures.remove(frame.texture);
        }
    }

    fn submit(&self, commands: CommandList) -> Result<(), RhiError> {
        if self.is_lost() {
            return Err(RhiError::DeviceLost);
        }
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let buffers = encode::encode_list(self, commands)?;
        self.queue.submit(buffers);
        match poll::ready(scope.pop()) {
            Some(Some(error)) => Err(RhiError::Backend(error.to_string())),
            _ => Ok(()),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn read_buffer(&self, buffer: BufferId) -> Result<Vec<u8>, RhiError> {
        let buffer = self
            .lock()
            .buffers
            .get(buffer)
            .cloned()
            .ok_or(RhiError::InvalidHandle("buffer"))?;
        let slice = buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|error| RhiError::Backend(error.to_string()))?;
        receiver
            .recv()
            .map_err(|_| RhiError::Backend("buffer mapping callback never ran".into()))?
            .map_err(|error| RhiError::Backend(error.to_string()))?;
        let data = slice
            .get_mapped_range()
            .map_err(|error| RhiError::Backend(error.to_string()))?
            .to_vec();
        buffer.unmap();
        Ok(data)
    }

    #[cfg(target_arch = "wasm32")]
    fn read_buffer(&self, _buffer: BufferId) -> Result<Vec<u8>, RhiError> {
        Err(RhiError::Unsupported("blocking buffer readback on the web"))
    }

    fn wait_idle(&self) -> Result<(), RhiError> {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map(|_| ())
            .map_err(|error| RhiError::Backend(error.to_string()))
    }
}

fn pipeline_layout(
    device: &wgpu::Device,
    objects: &Objects,
    label: &str,
    layouts: &[BindGroupLayoutId],
) -> Result<wgpu::PipelineLayout, RhiError> {
    let mut bind_group_layouts = Vec::with_capacity(layouts.len());
    for layout in layouts {
        bind_group_layouts.push(Some(
            objects
                .layouts
                .get(*layout)
                .ok_or(RhiError::InvalidHandle("bind group layout"))?,
        ));
    }
    Ok(device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &bind_group_layouts,
        immediate_size: 0,
    }))
}

#[cfg(test)]
mod surface_format_tests {
    use wgpu::TextureFormat as F;

    use super::*;

    fn chosen(wanted: Option<F>, available: &[F]) -> Option<(F, F)> {
        choose_surface_format(wanted, available).map(|chosen| (chosen.configured, chosen.view))
    }

    #[test]
    fn native_srgb_is_kept_without_view_formats() {
        let picked = choose_surface_format(None, &[F::Bgra8Unorm, F::Bgra8UnormSrgb]).unwrap();
        assert_eq!((picked.configured, picked.view), (F::Bgra8UnormSrgb, F::Bgra8UnormSrgb));
        assert_eq!(picked.view_formats(), Vec::<wgpu::TextureFormat>::new());
    }

    #[test]
    fn web_unorm_is_viewed_as_srgb() {
        let picked = choose_surface_format(None, &[F::Bgra8Unorm, F::Rgba8Unorm]).unwrap();
        assert_eq!((picked.configured, picked.view), (F::Bgra8Unorm, F::Bgra8UnormSrgb));
        assert_eq!(picked.view_formats(), vec![F::Bgra8UnormSrgb]);
    }

    #[test]
    fn explicit_supported_format_is_honoured() {
        assert_eq!(
            chosen(Some(F::Rgba8Unorm), &[F::Bgra8Unorm, F::Rgba8Unorm]),
            Some((F::Rgba8Unorm, F::Rgba8Unorm))
        );
    }

    #[test]
    fn unsupported_wanted_falls_back_to_default_choice() {
        assert_eq!(
            chosen(Some(F::Rgba16Float), &[F::Bgra8Unorm]),
            Some((F::Bgra8Unorm, F::Bgra8UnormSrgb))
        );
    }

    #[test]
    fn format_without_srgb_counterpart_is_used_as_is() {
        assert_eq!(chosen(None, &[F::Rgba16Float]), Some((F::Rgba16Float, F::Rgba16Float)));
    }

    #[test]
    fn nothing_offered_yields_none() {
        assert_eq!(chosen(None, &[]), None);
    }
}
