use pathon_api::Slot;

/// Which of the base slots (docs/slots.md) a preview shows. `Slot::OUTPUT` is missing on purpose: the preview is
/// drawn into it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PreviewSlot {
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
}

/// The kind of texture binding a slot needs, so that one shader module fits several slots.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum SourceKind {
    Float,
    Depth,
    Uint,
}

/// How the shader turns a texel into a colour. The numbers are the `MODE_*` constants of `preview_common.wgsl`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u32)]
pub(crate) enum PreviewMode {
    Scalar = 0,
    LinearColor = 1,
    ExposedHdr = 2,
    Depth = 3,
    Normals = 4,
    Motion = 5,
    MaterialId = 6,
    ExposureValue = 7,
}

impl PreviewSlot {
    pub const ALL: [Self; 13] = [
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
    ];

    pub const fn slot(self) -> Slot {
        match self {
            Self::Depth => Slot::DEPTH,
            Self::DepthPyramid => Slot::DEPTH_PYRAMID,
            Self::Normals => Slot::NORMALS,
            Self::Motion => Slot::MOTION,
            Self::MaterialId => Slot::MATERIAL_ID,
            Self::Roughness => Slot::ROUGHNESS,
            Self::Albedo => Slot::ALBEDO,
            Self::Reactive => Slot::REACTIVE,
            Self::Diffuse => Slot::DIFFUSE,
            Self::Specular => Slot::SPECULAR,
            Self::Hdr => Slot::HDR,
            Self::Ao => Slot::AO,
            Self::Exposure => Slot::EXPOSURE,
        }
    }

    pub(crate) const fn kind(self) -> SourceKind {
        match self {
            Self::Depth => SourceKind::Depth,
            Self::MaterialId => SourceKind::Uint,
            _ => SourceKind::Float,
        }
    }

    pub(crate) const fn mode(self) -> PreviewMode {
        match self {
            Self::Depth | Self::DepthPyramid => PreviewMode::Depth,
            Self::Normals => PreviewMode::Normals,
            Self::Motion => PreviewMode::Motion,
            Self::MaterialId => PreviewMode::MaterialId,
            Self::Roughness | Self::Reactive | Self::Ao => PreviewMode::Scalar,
            Self::Albedo => PreviewMode::LinearColor,
            Self::Diffuse | Self::Specular | Self::Hdr => PreviewMode::ExposedHdr,
            Self::Exposure => PreviewMode::ExposureValue,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PreviewLayout {
    /// The first slot fills `Slot::OUTPUT`.
    Fullscreen,
    /// The first slot in a corner, on top of whatever the output holds.
    Corner(Corner),
    /// Every slot in a grid, row by row in list order, over a dark background.
    Grid,
}

/// What to look at: any base slot, in `Slot::OUTPUT`. The pack requests on-demand slots in `prepare_view` and never
/// produces a slot itself: when no other pack writes it, the preview shows a checkered tile and reports U0001.
#[derive(Clone, PartialEq, Debug)]
pub struct SlotPreview {
    pub slots: Vec<PreviewSlot>,
    pub layout: PreviewLayout,
    /// Corner layout: width of the preview as a fraction of the output.
    pub corner_size: f32,
    /// Depth and depth pyramid: view distance in metres mapped to the heat ramp (near is bright).
    pub depth_range: (f32, f32),
    /// Depth pyramid: the mip level shown (clamped to the last level).
    pub pyramid_level: u32,
    /// Motion: the length in pixels that is shown at full brightness.
    pub motion_scale: f32,
    /// Hdr, diffuse and specular are multiplied by this before they are clamped to the display range.
    pub exposure: f32,
    /// Use `Slot::EXPOSURE` instead of `exposure` when some pack wrote it.
    pub exposure_from_slot: bool,
}

impl SlotPreview {
    pub fn new(slots: Vec<PreviewSlot>, layout: PreviewLayout) -> Self {
        Self {
            slots,
            layout,
            corner_size: 0.3,
            depth_range: (0.1, 20.0),
            pyramid_level: 0,
            motion_scale: 8.0,
            exposure: 1.0,
            exposure_from_slot: false,
        }
    }

    pub fn fullscreen(slot: PreviewSlot) -> Self {
        Self::new(vec![slot], PreviewLayout::Fullscreen)
    }

    pub fn corner(slot: PreviewSlot, corner: Corner) -> Self {
        Self::new(vec![slot], PreviewLayout::Corner(corner))
    }

    pub fn grid(slots: Vec<PreviewSlot>) -> Self {
        Self::new(slots, PreviewLayout::Grid)
    }

    /// Slots that are drawn: one for the fullscreen and corner layouts, all of them for the grid.
    pub(crate) fn visible_slots(&self) -> &[PreviewSlot] {
        match self.layout {
            PreviewLayout::Grid => &self.slots,
            PreviewLayout::Fullscreen | PreviewLayout::Corner(_) => &self.slots[..self.slots.len().min(1)],
        }
    }

    pub(crate) fn sanitized(&self) -> (Self, Vec<String>) {
        let mut settings = self.clone();
        let mut problems = Vec::new();
        let mut repair = |what: &str, ok: bool, fix: &mut dyn FnMut()| {
            if !ok {
                problems.push(what.to_owned());
                fix();
            }
        };
        repair(
            "corner_size outside 0.05..=1",
            self.corner_size.is_finite() && (0.05..=1.0).contains(&self.corner_size),
            &mut || {
                settings.corner_size = if self.corner_size.is_finite() {
                    self.corner_size.clamp(0.05, 1.0)
                } else {
                    0.3
                }
            },
        );
        let (near, far) = self.depth_range;
        repair(
            "depth_range is not 0 <= near < far",
            near.is_finite() && far.is_finite() && near >= 0.0 && far > near,
            &mut || settings.depth_range = (0.1, 20.0),
        );
        repair(
            "motion_scale is not positive",
            self.motion_scale.is_finite() && self.motion_scale > 0.0,
            &mut || settings.motion_scale = 8.0,
        );
        repair(
            "exposure is not a positive number",
            self.exposure.is_finite() && self.exposure > 0.0,
            &mut || settings.exposure = 1.0,
        );
        (settings, problems)
    }
}

/// Which wireframe path draws `DebugDraw::wireframe` meshes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum WireframeMode {
    /// Hardware line polygon mode when the device has `POLYGON_MODE_LINE`, otherwise barycentric.
    #[default]
    Auto,
    /// Always the barycentric shader (what the Web gets).
    Barycentric,
}

/// Read from `Shared` every frame, so the application changes it between frames.
#[derive(Clone, PartialEq, Debug)]
pub struct DebugSettings {
    pub lines: bool,
    pub wireframe: WireframeMode,
    /// Wireframes are hidden behind `Slot::DEPTH` when some pack writes it.
    pub wireframe_depth_test: bool,
    pub preview: Option<SlotPreview>,
    /// Relative depth tolerance of depth-tested lines: a line is hidden where the scene is nearer by more than this
    /// fraction of its depth.
    pub depth_bias: f32,
}

impl Default for DebugSettings {
    fn default() -> Self {
        Self {
            lines: true,
            wireframe: WireframeMode::Auto,
            wireframe_depth_test: true,
            preview: None,
            depth_bias: 1e-3,
        }
    }
}

impl DebugSettings {
    pub(crate) fn depth_bias(&self) -> f32 {
        if self.depth_bias.is_finite() {
            self.depth_bias.clamp(0.0, 0.5)
        } else {
            1e-3
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_base_slot_but_output_has_a_preview_and_each_maps_to_its_own_slot() {
        let slots: Vec<Slot> = PreviewSlot::ALL.iter().map(|preview| preview.slot()).collect();
        for builtin in Slot::BUILT_IN {
            assert_eq!(slots.contains(&builtin), builtin != Slot::OUTPUT, "{builtin:?}");
        }
    }

    #[test]
    fn the_source_kind_matches_the_slot_format() {
        use pathon_api::rhi::Format;
        for preview in PreviewSlot::ALL {
            let format = preview.slot().format().expect("every previewed slot has a format");
            let expected = match format {
                Format::Depth32Float => SourceKind::Depth,
                Format::R32Uint => SourceKind::Uint,
                _ => SourceKind::Float,
            };
            assert_eq!(preview.kind(), expected, "{preview:?}");
        }
    }

    #[test]
    fn fullscreen_and_corner_show_one_slot_and_the_grid_all_of_them() {
        let slots = vec![PreviewSlot::Depth, PreviewSlot::Ao];
        let mut preview = SlotPreview::new(slots.clone(), PreviewLayout::Fullscreen);
        assert_eq!(preview.visible_slots(), [PreviewSlot::Depth]);
        preview.layout = PreviewLayout::Corner(Corner::TopLeft);
        assert_eq!(preview.visible_slots(), [PreviewSlot::Depth]);
        preview.layout = PreviewLayout::Grid;
        assert_eq!(preview.visible_slots(), slots);
        assert_eq!(SlotPreview::fullscreen(PreviewSlot::Hdr).slots.len(), 1);
    }

    #[test]
    fn broken_numbers_are_repaired_and_named() {
        let mut preview = SlotPreview::fullscreen(PreviewSlot::Hdr);
        preview.corner_size = 7.0;
        preview.depth_range = (5.0, 1.0);
        preview.motion_scale = f32::NAN;
        preview.exposure = -1.0;

        let (repaired, problems) = preview.sanitized();

        assert_eq!(problems.len(), 4, "{problems:?}");
        assert_eq!(repaired.corner_size, 1.0);
        assert_eq!(repaired.depth_range, (0.1, 20.0));
        assert_eq!(repaired.motion_scale, 8.0);
        assert_eq!(repaired.exposure, 1.0);
        assert_eq!(
            SlotPreview::fullscreen(PreviewSlot::Hdr).sanitized().1,
            [] as [String; 0]
        );
    }
}
