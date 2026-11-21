use std::collections::HashMap;

use pathon_resources::DeferredRelease;
use pathon_rhi_types::{
    BufferDesc, BufferId, DeviceApi, RhiError, TextureDesc, TextureId, TextureViewDesc, TextureViewId,
};

// Textures unused for this many frames leave the pool.
const KEEP_UNUSED_FRAMES: u64 = 3;

#[derive(Debug)]
struct PooledTexture {
    texture: TextureId,
    desc: TextureDesc,
    last_used: u64,
    in_use: bool,
}

#[derive(Debug)]
struct PooledBuffer {
    buffer: BufferId,
    desc: BufferDesc,
    last_used: u64,
    in_use: bool,
}

// Physical textures and buffers that outlive one frame graph, so a stable scene allocates nothing per frame.
#[derive(Debug, Default)]
pub struct ResourcePool {
    textures: Vec<PooledTexture>,
    buffers: Vec<PooledBuffer>,
    views: HashMap<(TextureId, TextureViewDesc), TextureViewId>,
    created: usize,
}

impl ResourcePool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    pub fn created_total(&self) -> usize {
        self.created
    }

    pub(crate) fn begin_frame(&mut self) {
        for pooled in &mut self.textures {
            pooled.in_use = false;
        }
        for pooled in &mut self.buffers {
            pooled.in_use = false;
        }
    }

    pub(crate) fn acquire(
        &mut self,
        device: &dyn DeviceApi,
        desc: &TextureDesc,
        frame: u64,
    ) -> Result<TextureId, RhiError> {
        if let Some(pooled) = self
            .textures
            .iter_mut()
            .find(|pooled| !pooled.in_use && pooled.desc.same_allocation(desc))
        {
            pooled.in_use = true;
            pooled.last_used = frame;
            return Ok(pooled.texture);
        }
        let texture = device.create_texture(desc)?;
        self.created += 1;
        self.textures.push(PooledTexture {
            texture,
            desc: *desc,
            last_used: frame,
            in_use: true,
        });
        Ok(texture)
    }

    pub(crate) fn acquire_buffer(
        &mut self,
        device: &dyn DeviceApi,
        desc: &BufferDesc,
        frame: u64,
    ) -> Result<BufferId, RhiError> {
        if let Some(pooled) = self
            .buffers
            .iter_mut()
            .find(|pooled| !pooled.in_use && pooled.desc.size == desc.size && pooled.desc.usage == desc.usage)
        {
            pooled.in_use = true;
            pooled.last_used = frame;
            return Ok(pooled.buffer);
        }
        let buffer = device.create_buffer(desc)?;
        self.created += 1;
        self.buffers.push(PooledBuffer {
            buffer,
            desc: *desc,
            last_used: frame,
            in_use: true,
        });
        Ok(buffer)
    }

    pub(crate) fn view(
        &mut self,
        device: &dyn DeviceApi,
        texture: TextureId,
        desc: &TextureViewDesc,
    ) -> Result<TextureViewId, RhiError> {
        if let Some(view) = self.views.get(&(texture, *desc)) {
            return Ok(*view);
        }
        let view = device.create_texture_view(texture, desc)?;
        self.views.insert((texture, *desc), view);
        Ok(view)
    }

    pub fn release_views_of(&mut self, texture: TextureId, release: &mut DeferredRelease, frame: u64) {
        self.views.retain(|(owner, _), view| {
            let keep = *owner != texture;
            if !keep {
                release.release(frame, *view);
            }
            keep
        });
    }

    pub(crate) fn end_frame(&mut self, frame: u64, release: &mut DeferredRelease) {
        let mut expired = Vec::new();
        self.textures.retain(|pooled| {
            let keep = pooled.in_use || pooled.last_used + KEEP_UNUSED_FRAMES > frame;
            if !keep {
                expired.push(pooled.texture);
            }
            keep
        });
        self.buffers.retain(|pooled| {
            let keep = pooled.in_use || pooled.last_used + KEEP_UNUSED_FRAMES > frame;
            if !keep {
                release.release(frame, pooled.buffer);
            }
            keep
        });
        for texture in expired {
            self.release_views_of(texture, release, frame);
            release.release(frame, texture);
        }
    }

    // After device loss every handle is dead; drop them without destroying.
    pub fn forget_all(&mut self) {
        self.textures.clear();
        self.buffers.clear();
        self.views.clear();
    }
}
