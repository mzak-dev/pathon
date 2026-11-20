use pathon_rhi::{
    Access, DeviceApi, Format, Id, PhysicalSize, RawHandle, RhiError, SurfaceConfig, SurfaceId, SurfaceTarget,
    TextureDesc, TextureId, TextureUsage,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TargetId(RawHandle);

impl Id for TargetId {
    const KIND: &'static str = "render target";

    fn from_raw(raw: RawHandle) -> Self {
        Self(raw)
    }

    fn raw(self) -> RawHandle {
        self.0
    }
}

#[derive(Debug)]
pub(crate) enum TargetKind {
    // `window` is kept only when pathon created the surface, so it can rebuild it after device loss.
    Surface {
        surface: SurfaceId,
        window: Option<SurfaceTarget>,
        config: SurfaceConfig,
    },
    Offscreen {
        texture: TextureId,
        desc: TextureDesc,
        state: Access,
    },
    Imported {
        texture: TextureId,
        desc: TextureDesc,
        before: Access,
        after: Access,
    },
}

#[derive(Debug)]
pub(crate) struct Target {
    pub(crate) kind: TargetKind,
    pub(crate) size: PhysicalSize,
}

pub(crate) const OFFSCREEN_USAGE: TextureUsage = TextureUsage::RENDER_TARGET
    .union(TextureUsage::SAMPLED)
    .union(TextureUsage::COPY_SRC)
    .union(TextureUsage::COPY_DST);

// Offscreen targets end every frame readable, so hosts can sample or copy them.
pub(crate) const OFFSCREEN_FINAL: Access = Access::SHADER_READ.union(Access::COPY_SRC);

pub(crate) fn offscreen_desc(size: PhysicalSize, format: Format) -> TextureDesc {
    TextureDesc::new_2d(
        "offscreen target",
        size.width.max(1),
        size.height.max(1),
        format,
        OFFSCREEN_USAGE,
    )
}

pub(crate) fn configure(
    device: &dyn DeviceApi,
    surface: SurfaceId,
    config: &mut SurfaceConfig,
) -> Result<(), RhiError> {
    match device.configure_surface(surface, config) {
        Ok(format) => {
            config.format = Some(format);
            Ok(())
        }
        // Minimised windows have no size; the surface is configured on the next resize.
        Err(RhiError::SurfaceUnavailable) => Ok(()),
        Err(error) => Err(error),
    }
}
