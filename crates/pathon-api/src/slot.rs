use std::hash::{Hash, Hasher};

use pathon_graph::TextureHandle;
use pathon_rhi_types::Format;

// A named texture packs exchange within a view. The built-in slots form the contract in docs/slots.md; any pack can
// declare its own (`Slot::new("my_pack.velocity", Format::Rg16Float)`) for other packs to read. Slots are equal when
// their names are.
#[derive(Clone, Copy, Debug)]
pub struct Slot {
    name: &'static str,
    format: Option<Format>,
    on_demand: bool,
}

impl PartialEq for Slot {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for Slot {}

impl Hash for Slot {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl Slot {
    pub const DEPTH: Self = Self::new("depth", Format::Depth32Float);
    pub const DEPTH_PYRAMID: Self = Self::new("depth_pyramid", Format::R32Float).on_demand();
    pub const NORMALS: Self = Self::new("normals", Format::Rg16Float);
    pub const MOTION: Self = Self::new("motion", Format::Rg16Float);
    pub const MATERIAL_ID: Self = Self::new("material_id", Format::R32Uint);
    pub const ROUGHNESS: Self = Self::new("roughness", Format::R8Unorm).on_demand();
    pub const ALBEDO: Self = Self::new("albedo", Format::Rgba8Unorm).on_demand();
    pub const REACTIVE: Self = Self::new("reactive", Format::R8Unorm).on_demand();
    pub const DIFFUSE: Self = Self::new("diffuse", Format::Rgba16Float);
    pub const SPECULAR: Self = Self::new("specular", Format::Rgba16Float);
    pub const HDR: Self = Self::new("hdr", Format::Rgba16Float);
    pub const AO: Self = Self::new("ao", Format::R8Unorm);
    pub const EXPOSURE: Self = Self::new("exposure", Format::R32Float);
    // The render target itself; its format is the target's.
    pub const OUTPUT: Self = Self {
        name: "output",
        format: None,
        on_demand: false,
    };

    pub const BUILT_IN: [Self; 14] = [
        Self::DEPTH,
        Self::DEPTH_PYRAMID,
        Self::NORMALS,
        Self::MOTION,
        Self::MATERIAL_ID,
        Self::ROUGHNESS,
        Self::ALBEDO,
        Self::REACTIVE,
        Self::DIFFUSE,
        Self::SPECULAR,
        Self::HDR,
        Self::AO,
        Self::EXPOSURE,
        Self::OUTPUT,
    ];

    pub const fn new(name: &'static str, format: Format) -> Self {
        Self {
            name,
            format: Some(format),
            on_demand: false,
        }
    }

    // Produced only when a consumer asks for it in `prepare_view`.
    #[must_use]
    pub const fn on_demand(self) -> Self {
        Self {
            on_demand: true,
            ..self
        }
    }

    pub const fn name(self) -> &'static str {
        self.name
    }

    pub const fn format(self) -> Option<Format> {
        self.format
    }

    pub const fn is_on_demand(self) -> bool {
        self.on_demand
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
