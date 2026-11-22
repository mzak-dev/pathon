use pathon_api::OutputInfo;
use pathon_diag::Diagnostic;
use pathon_rhi::{
    Access, DeviceApi, Format, Id, PhysicalSize, RawHandle, RhiError, SurfaceColorSpace, SurfaceConfig, SurfaceId,
    SurfaceTarget, TextureDesc, TextureId, TextureUsage,
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
        // `config.color_space` is what was asked for; `output` what the surface presents after the last configure.
        config: SurfaceConfig,
        output: OutputInfo,
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
pub(crate) const OFFSCREEN_FINAL: Access = Access::SAMPLED;

pub(crate) fn offscreen_desc(size: PhysicalSize, format: Format) -> TextureDesc {
    TextureDesc::new_2d(
        "offscreen target",
        size.width.max(1),
        size.height.max(1),
        format,
        OFFSCREEN_USAGE,
    )
}

// Returns H0001 when the surface could not present the colour space that was asked for.
pub(crate) fn configure(
    device: &dyn DeviceApi,
    surface: SurfaceId,
    config: &mut SurfaceConfig,
    output: &mut OutputInfo,
) -> Result<Option<Diagnostic>, RhiError> {
    match device.configure_surface(surface, config) {
        Ok(configured) => {
            config.format = Some(configured.format);
            *output = OutputInfo {
                color_space: configured.color_space,
                display: configured.display,
            };
            Ok(unsupported_color_space(config.color_space, configured.color_space))
        }
        // Minimised windows have no size; the surface is configured on the next resize.
        Err(RhiError::SurfaceUnavailable) => Ok(None),
        Err(error) => Err(error),
    }
}

fn unsupported_color_space(requested: SurfaceColorSpace, presented: SurfaceColorSpace) -> Option<Diagnostic> {
    if requested == SurfaceColorSpace::Auto || requested == presented {
        return None;
    }
    Some(
        Diagnostic::warning(
            "H0001",
            format!("the surface cannot present colour space {requested:?}; presenting {presented:?} instead"),
        )
        .with_hint("Renderer::target_surface_caps lists what the surface offers; HDR needs the OS to have HDR on"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_space_that_was_replaced_is_reported() {
        let srgb = SurfaceColorSpace::Srgb;
        let scrgb = SurfaceColorSpace::ExtendedSrgbLinear;
        assert!(unsupported_color_space(SurfaceColorSpace::Auto, srgb).is_none());
        assert!(unsupported_color_space(srgb, srgb).is_none());
        assert!(unsupported_color_space(scrgb, scrgb).is_none());
        let diagnostic = unsupported_color_space(scrgb, srgb).expect("a fallback is reported");
        assert_eq!(diagnostic.code, "H0001");
    }
}
