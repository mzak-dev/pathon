// Raw wgpu access. Only vendor packs and host adapters may depend on this crate (`xtask deps-check`).

use pathon_rhi_types::{
    CommandList, DeclaredAccess, DeviceApi, ResourceRef, RhiError, TextureDesc, TextureDimension, TextureId,
    TextureViewDesc, TextureViewId,
};

use crate::device::{Objects, TextureEntry, WgpuDevice};
use crate::{convert, encode};

// Put into `DeviceRequest::extensions` to edit the wgpu device descriptor before the device is created.
#[derive(Clone, Copy, Debug)]
pub struct DeviceHook(pub fn(&mut wgpu::DeviceDescriptor<'static>));

// What an `ExternalCommand` callback receives on this backend; downcast it from `&mut dyn Any`.
#[derive(Debug)]
pub struct ExternalContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub encoder: wgpu::CommandEncoder,
    textures: Vec<(TextureId, wgpu::Texture)>,
}

impl ExternalContext {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        objects: &Objects,
        accesses: &[DeclaredAccess],
    ) -> Self {
        let textures = accesses
            .iter()
            .filter_map(|declared| match declared.resource {
                ResourceRef::Texture(id, _) => objects.textures.get(id).map(|entry| (id, entry.texture.clone())),
                ResourceRef::Buffer(_) => None,
                other => {
                    log::error!("external command declares {other:?}, which the wgpu backend cannot track");
                    None
                }
            })
            .collect();
        let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pathon external command"),
        });
        Self {
            device: device.clone(),
            queue: queue.clone(),
            encoder,
            textures,
        }
    }

    pub fn texture(&self, id: TextureId) -> Option<&wgpu::Texture> {
        self.textures
            .iter()
            .find(|(candidate, _)| *candidate == id)
            .map(|(_, texture)| texture)
    }

    pub(crate) fn finish(self) -> wgpu::CommandBuffer {
        self.encoder.finish()
    }
}

// Build a pathon device on top of a device the host already owns (integration modes A and B, ADR-0005).
pub fn adopt(instance: wgpu::Instance, adapter: wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue) -> WgpuDevice {
    WgpuDevice::from_parts(instance, adapter, device, queue, None)
}

impl WgpuDevice {
    pub fn raw_instance(&self) -> &wgpu::Instance {
        self.instance()
    }

    pub fn raw_adapter(&self) -> &wgpu::Adapter {
        self.adapter()
    }

    pub fn raw_device(&self) -> &wgpu::Device {
        self.device()
    }

    pub fn raw_queue(&self) -> &wgpu::Queue {
        self.queue()
    }

    pub fn raw_texture(&self, texture: TextureId) -> Option<wgpu::Texture> {
        self.lock().textures.get(texture).map(|entry| entry.texture.clone())
    }

    pub fn raw_texture_view(&self, view: TextureViewId) -> Option<wgpu::TextureView> {
        self.lock().views.get(view).cloned()
    }

    // Registers a texture owned by the host so the frame graph can import it. `forget_texture` unregisters it
    // without destroying it.
    pub fn import_texture(&self, label: &'static str, texture: wgpu::Texture) -> Result<TextureId, RhiError> {
        let format = convert::format_from_wgpu(texture.format()).ok_or(RhiError::Unsupported("texture format"))?;
        let size = texture.size();
        let desc = TextureDesc {
            label,
            size: pathon_rhi_types::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_layers: size.depth_or_array_layers,
            },
            mip_level_count: texture.mip_level_count(),
            sample_count: texture.sample_count(),
            dimension: match texture.dimension() {
                wgpu::TextureDimension::D1 => TextureDimension::D1,
                wgpu::TextureDimension::D2 => TextureDimension::D2,
                wgpu::TextureDimension::D3 => TextureDimension::D3,
            },
            format,
            usage: convert::texture_usage_from_wgpu(texture.usage()),
            // wgpu does not expose the view formats of a texture the host created.
            view_formats: &[],
        };
        Ok(self.lock().textures.insert(TextureEntry { texture, desc }))
    }

    pub fn forget_texture(&self, texture: TextureId) {
        self.lock().textures.remove(texture);
    }

    pub fn import_texture_view(&self, texture: TextureId) -> Result<TextureViewId, RhiError> {
        self.create_texture_view(texture, &TextureViewDesc::default())
    }

    // Records a pathon command list into an encoder owned by the host; the host submits it (mode A, ADR-0005).
    pub fn encode_into(&self, list: CommandList, encoder: &mut wgpu::CommandEncoder) -> Result<(), RhiError> {
        let objects = self.lock();
        encode::encode_into(&objects, encoder, list, self.caps().features)
    }
}
