use std::fmt;
use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::color_space::SurfaceColorSpace;
use crate::format::Format;
use crate::id::{TextureId, TextureViewId};
use crate::maybe_send::MaybeSendSync;

pub trait WindowSource: HasWindowHandle + HasDisplayHandle + MaybeSendSync {}

impl<T: HasWindowHandle + HasDisplayHandle + MaybeSendSync + ?Sized> WindowSource for T {}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct PhysicalSize {
    pub width: u32,
    pub height: u32,
}

impl PhysicalSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

#[derive(Clone)]
pub struct SurfaceTarget {
    window: Arc<dyn WindowSource>,
    pub size: PhysicalSize,
    pub scale_factor: f64,
}

impl SurfaceTarget {
    pub fn new(window: Arc<dyn WindowSource>, size: PhysicalSize, scale_factor: f64) -> Self {
        Self {
            window,
            size,
            scale_factor,
        }
    }

    pub fn window(&self) -> &Arc<dyn WindowSource> {
        &self.window
    }
}

impl fmt::Debug for SurfaceTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SurfaceTarget")
            .field("size", &self.size)
            .field("scale_factor", &self.scale_factor)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum PresentMode {
    #[default]
    Vsync,
    LowLatency,
    Immediate,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SurfaceConfig {
    pub size: PhysicalSize,
    pub present_mode: PresentMode,
    pub format: Option<Format>,
    pub color_space: SurfaceColorSpace,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SurfaceFrame {
    pub texture: TextureId,
    pub view: TextureViewId,
    pub format: Format,
    pub size: PhysicalSize,
}
