use pathon_api::rhi::{AddressMode, FilterMode, SamplerDesc};

/// One of the fixed samplers every texture of a material can choose from. Samplers are separate bindings, not a
/// `binding_array<sampler>`, so the limit of sampler arrays (zero on most devices) never matters.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum SamplerPreset {
    #[default]
    LinearRepeat,
    LinearClamp,
    NearestRepeat,
    NearestClamp,
}

impl SamplerPreset {
    pub const ALL: [Self; 4] = [
        Self::LinearRepeat,
        Self::LinearClamp,
        Self::NearestRepeat,
        Self::NearestClamp,
    ];
    pub const COUNT: usize = Self::ALL.len();

    pub fn index(self) -> u32 {
        self as u32
    }

    pub fn from_index(index: u32) -> Option<Self> {
        Self::ALL.get(index as usize).copied()
    }

    // No anisotropy: the same pixels on every GPU keep the golden images of the two paths comparable.
    pub fn desc(self) -> SamplerDesc {
        let (label, address_mode, filter) = match self {
            Self::LinearRepeat => ("material linear repeat", AddressMode::Repeat, FilterMode::Linear),
            Self::LinearClamp => ("material linear clamp", AddressMode::ClampToEdge, FilterMode::Linear),
            Self::NearestRepeat => ("material nearest repeat", AddressMode::Repeat, FilterMode::Nearest),
            Self::NearestClamp => ("material nearest clamp", AddressMode::ClampToEdge, FilterMode::Nearest),
        };
        SamplerDesc {
            label,
            address_mode,
            mag_filter: filter,
            min_filter: filter,
            mipmap_filter: filter,
            ..SamplerDesc::LINEAR_CLAMP
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_round_trip() {
        for preset in SamplerPreset::ALL {
            assert_eq!(SamplerPreset::from_index(preset.index()), Some(preset));
        }
        assert_eq!(
            SamplerPreset::from_index(u32::try_from(SamplerPreset::COUNT).expect("four presets")),
            None
        );
    }
}
