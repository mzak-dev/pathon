use pathon_api::rhi::{Access, DeviceApi, Format, RhiError, TextureDesc, TextureId, TextureViewDesc, TextureViewId};

use crate::choice::FormatChoice;

/// A loaded GPU texture: every mip is written and the whole texture is in [`Texture::STATE`].
#[derive(Clone, Copy, Debug)]
pub struct Texture {
    pub id: TextureId,
    pub desc: TextureDesc,
    pub choice: FormatChoice,
}

impl Texture {
    /// Where the loader leaves every subresource, for the `initial` state of an import into the frame graph.
    pub const STATE: Access = Access::SAMPLED;

    pub fn sampling_format(&self) -> Format {
        self.choice.sampling
    }

    /// A view that decodes the texture the way its [`ColorSpace`](crate::ColorSpace) says; `dimension` is left to the
    /// texture (set `Cube` on a six-layer cube map).
    pub fn view_desc(&self) -> TextureViewDesc {
        TextureViewDesc {
            format: (self.choice.sampling != self.desc.format).then_some(self.choice.sampling),
            ..TextureViewDesc::default()
        }
    }

    pub fn create_view(&self, device: &dyn DeviceApi) -> Result<TextureViewId, RhiError> {
        device.create_texture_view(self.id, &self.view_desc())
    }
}

// `TextureDesc` keeps its view formats as a static slice so it stays `Copy`; 8-bit colour is the only case that needs one.
pub(crate) fn view_formats_for(storage: Format, sampling: Format) -> &'static [Format] {
    match (storage, sampling) {
        (Format::Rgba8Unorm, Format::Rgba8UnormSrgb) => &[Format::Rgba8UnormSrgb],
        (Format::Bgra8Unorm, Format::Bgra8UnormSrgb) => &[Format::Bgra8UnormSrgb],
        _ => &[],
    }
}
