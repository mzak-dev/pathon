use pathon_graph::TextureHandle;
use pathon_rhi_types::Format;

// The slot contract (docs/slots.md): what each slot holds and in which format. Host adapters convert their buffers to
// this contract; packs never see host conventions.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Slot {
    Depth,
    DepthPyramid,
    Normals,
    Motion,
    MaterialId,
    Roughness,
    Albedo,
    Reactive,
    Diffuse,
    Specular,
    Hdr,
    Ao,
    Exposure,
    Output,
}

impl Slot {
    pub const ALL: [Self; 14] = [
        Self::Depth,
        Self::DepthPyramid,
        Self::Normals,
        Self::Motion,
        Self::MaterialId,
        Self::Roughness,
        Self::Albedo,
        Self::Reactive,
        Self::Diffuse,
        Self::Specular,
        Self::Hdr,
        Self::Ao,
        Self::Exposure,
        Self::Output,
    ];

    // `None` for `Output`: its format is the render target's.
    pub fn format(self) -> Option<Format> {
        let format = match self {
            Self::Depth => Format::Depth32Float,
            Self::DepthPyramid | Self::Exposure => Format::R32Float,
            Self::Normals | Self::Motion => Format::Rg16Float,
            Self::MaterialId => Format::R32Uint,
            Self::Roughness | Self::Reactive | Self::Ao => Format::R8Unorm,
            Self::Albedo => Format::Rgba8Unorm,
            Self::Diffuse | Self::Specular | Self::Hdr => Format::Rgba16Float,
            Self::Output => return None,
        };
        Some(format)
    }

    // Produced only when a consumer asks for it in `prepare_view`.
    pub fn is_on_demand(self) -> bool {
        matches!(
            self,
            Self::Roughness | Self::Albedo | Self::DepthPyramid | Self::Reactive
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct Slots {
    entries: Vec<(Slot, TextureHandle)>,
}

impl Slots {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, slot: Slot) -> Option<TextureHandle> {
        self.entries
            .iter()
            .find(|(candidate, _)| *candidate == slot)
            .map(|(_, handle)| *handle)
    }

    pub(crate) fn insert(&mut self, slot: Slot, handle: TextureHandle) {
        if let Some(entry) = self.entries.iter_mut().find(|(candidate, _)| *candidate == slot) {
            entry.1 = handle;
        } else {
            self.entries.push((slot, handle));
        }
    }
}
