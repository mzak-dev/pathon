#![doc = include_str!("../../../docs/crates/pathon-rhi.md")]

pub use pathon_rhi_null::NullDevice;
pub use pathon_rhi_types::*;
pub use pathon_rhi_wgpu::WgpuDevice;

#[derive(Debug)]
pub enum Device {
    Wgpu(WgpuDevice),
    Null(NullDevice),
}

impl Device {
    pub async fn build_async(request: &DeviceRequest) -> Result<Self, RhiError> {
        match request.backend {
            Some(Backend::Null) => Ok(Self::Null(NullDevice::new())),
            _ => Ok(Self::Wgpu(WgpuDevice::new(request).await?)),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn build(request: &DeviceRequest) -> Result<Self, RhiError> {
        match request.backend {
            Some(Backend::Null) => Ok(Self::Null(NullDevice::new())),
            _ => Ok(Self::Wgpu(WgpuDevice::new_blocking(request)?)),
        }
    }

    pub fn null() -> Self {
        Self::Null(NullDevice::new())
    }

    pub fn as_wgpu(&self) -> Option<&WgpuDevice> {
        match self {
            Self::Wgpu(device) => Some(device),
            Self::Null(_) => None,
        }
    }

    pub fn as_null(&self) -> Option<&NullDevice> {
        match self {
            Self::Null(device) => Some(device),
            Self::Wgpu(_) => None,
        }
    }
}

impl From<WgpuDevice> for Device {
    fn from(device: WgpuDevice) -> Self {
        Self::Wgpu(device)
    }
}

impl From<NullDevice> for Device {
    fn from(device: NullDevice) -> Self {
        Self::Null(device)
    }
}

macro_rules! delegate {
    ($($name:ident(&self $(, $arg:ident: $ty:ty)*) -> $ret:ty;)*) => {
        impl DeviceApi for Device {
            $(
                fn $name(&self $(, $arg: $ty)*) -> $ret {
                    match self {
                        Self::Wgpu(device) => device.$name($($arg),*),
                        Self::Null(device) => device.$name($($arg),*),
                    }
                }
            )*
        }
    };
}

delegate! {
    caps(&self) -> &Caps;
    is_lost(&self) -> bool;
    create_buffer(&self, desc: &BufferDesc) -> Result<BufferId, RhiError>;
    write_buffer(&self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), RhiError>;
    create_texture(&self, desc: &TextureDesc) -> Result<TextureId, RhiError>;
    texture_desc(&self, texture: TextureId) -> Result<TextureDesc, RhiError>;
    write_texture(&self, dst: &TextureCopy, data: &[u8], layout: TexelLayout, size: Extent3d) -> Result<(), RhiError>;
    create_texture_view(&self, texture: TextureId, desc: &TextureViewDesc) -> Result<TextureViewId, RhiError>;
    create_sampler(&self, desc: &SamplerDesc) -> Result<SamplerId, RhiError>;
    create_shader_module(&self, desc: &ShaderModuleDesc<'_>) -> Result<ShaderModuleId, RhiError>;
    create_bind_group_layout(&self, desc: &BindGroupLayoutDesc) -> Result<BindGroupLayoutId, RhiError>;
    create_bind_group(&self, desc: &BindGroupDesc) -> Result<BindGroupId, RhiError>;
    create_render_pipeline(&self, desc: &RenderPipelineDesc) -> Result<RenderPipelineId, RhiError>;
    create_compute_pipeline(&self, desc: &ComputePipelineDesc) -> Result<ComputePipelineId, RhiError>;
    create_query_set(&self, desc: &QuerySetDesc) -> Result<QuerySetId, RhiError>;
    destroy(&self, resource: Resource) -> ();
    create_surface(&self, target: &SurfaceTarget) -> Result<SurfaceId, RhiError>;
    surface_caps(&self, surface: SurfaceId) -> Result<SurfaceCaps, RhiError>;
    configure_surface(&self, surface: SurfaceId, config: &SurfaceConfig) -> Result<SurfaceOutput, RhiError>;
    acquire_surface(&self, surface: SurfaceId) -> Result<SurfaceFrame, RhiError>;
    present_surface(&self, surface: SurfaceId) -> Result<(), RhiError>;
    discard_surface(&self, surface: SurfaceId) -> ();
    destroy_surface(&self, surface: SurfaceId) -> ();
    submit(&self, commands: CommandList) -> Result<(), RhiError>;
    read_buffer(&self, buffer: BufferId) -> Result<Vec<u8>, RhiError>;
    wait_idle(&self) -> Result<(), RhiError>;
    pipeline_cache_data(&self) -> Option<PipelineCacheData>;
}
