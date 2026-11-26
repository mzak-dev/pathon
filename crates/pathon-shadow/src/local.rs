// Shadows of point and spot lights, the CPU half: which lights get a map this frame, how large, where in the atlas,
// from which matrices, with which casters, and which of those maps still hold what they held last frame.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use pathon_api::glam::{Mat4, Vec3, Vec4};
use pathon_scene::{Aabb, Light, LightKind};

use crate::atlas::{Atlas, Tile};
use crate::input::ShadowCaster;
use crate::plan::{Batch, CasterGroups};
use crate::settings::ShadowSettings;

/// The widest spot cone (half angle) a map covers: beyond it the cone would need a field of view of more than 160
/// degrees, where a perspective map has no resolution left. Receivers outside the map are lit.
pub const MAX_SPOT_HALF_ANGLE: f32 = 80.0 * std::f32::consts::PI / 180.0;
/// Texels between the edge of what a map is sampled for and the edge of its tile, so that the filter never reads the
/// neighbouring tile.
const GUARD_TEXELS: f32 = 4.0;
/// Frames a map that no light asked for stays in the atlas (its content is reused when the light comes back).
const RETAIN_FRAMES: u64 = 240;

/// What shape of shadow map a light needs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LocalShadowShape {
    /// Six perspective maps of 90 degrees (plus a guard band), one per axis direction.
    Point,
    /// One perspective map covering the cone; `outer_angle` is the half angle in radians.
    Spot { outer_angle: f32 },
}

/// A point or spot light that may get a shadow map: the candidates the host puts in `ShadowScene::local_lights`; the
/// pack picks the ones that matter most for the view.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LocalShadowLight {
    /// Identity of the light across frames (a map is kept for as long as the key keeps being offered).
    pub key: u64,
    /// The index the lighting shader knows the light by (`shadow_factor_local` is asked about it).
    pub light_index: u32,
    pub shape: LocalShadowShape,
    pub position: Vec3,
    /// The unit vector a spot light shines along; ignored for a point light.
    pub direction: Vec3,
    /// Metres; the map covers this far.
    pub range: f32,
    /// How bright the light is, in any unit shared by all the lights (candela is natural): it ranks the lights.
    pub intensity: f32,
}

impl LocalShadowLight {
    /// `None` for a directional light. A point and a spot light become the matching shape; an area light is shadowed
    /// from its centre as a point light when `area_as_point` (an approximation: its penumbra is lost), else `None`.
    pub fn from_light(light: &Light, key: u64, light_index: u32, area_as_point: bool) -> Option<Self> {
        let shape = match light.kind {
            LightKind::Point => LocalShadowShape::Point,
            LightKind::Spot { outer_angle, .. } => LocalShadowShape::Spot { outer_angle },
            LightKind::Area { .. } if area_as_point => LocalShadowShape::Point,
            _ => return None,
        };
        let intensity = light
            .luminous_intensity_candela()
            .map_or(0.0, |candela| candela * light.color.max_element());
        Some(Self {
            key,
            light_index,
            shape,
            position: light.position(),
            direction: light.direction(),
            range: light.influence_radius()?,
            intensity,
        })
    }

    fn is_usable(&self) -> bool {
        let shape_ok = match self.shape {
            LocalShadowShape::Point => true,
            LocalShadowShape::Spot { outer_angle } => {
                outer_angle.is_finite() && outer_angle > 0.0 && self.direction.try_normalize().is_some()
            }
        };
        shape_ok
            && self.position.is_finite()
            && self.range.is_finite()
            && self.range > 0.0
            && self.intensity.is_finite()
            && self.intensity >= 0.0
    }

    fn tiles_needed(&self) -> usize {
        match self.shape {
            LocalShadowShape::Point => 6,
            LocalShadowShape::Spot { .. } => 1,
        }
    }

    // The indices of the light's tiles: faces for a point light, 0 for a spot light.
    fn faces(&self) -> std::ops::Range<u8> {
        0..u8::try_from(self.tiles_needed()).unwrap_or(6)
    }

    /// A sphere that contains everything the light reaches (the whole cone of a spot light).
    fn bounding_sphere(&self) -> (Vec3, f32) {
        match self.shape {
            LocalShadowShape::Point => (self.position, self.range),
            LocalShadowShape::Spot { outer_angle } => {
                let half = outer_angle.min(std::f32::consts::FRAC_PI_2 - 1.0e-3);
                let axis = self.direction.normalize_or_zero();
                if half <= std::f32::consts::FRAC_PI_4 {
                    // The smallest sphere through the apex and the rim of the base: centred on the axis at
                    // R / (2 cos^2) with that distance as its radius.
                    let reach = self.range / (2.0 * half.cos().powi(2));
                    (self.position + axis * reach, reach)
                } else {
                    (self.position + axis * self.range, self.range * half.tan())
                }
            }
        }
    }
}

/// The planes of a view volume, for culling: left, right, bottom, top, near and (when the projection has one) far, with
/// normals pointing inwards.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Frustum {
    planes: [Vec4; 6],
    count: usize,
}

impl Frustum {
    /// The volume of a reverse-Z, depth 0 to 1 clip space (what `Camera::projection` and the maps use).
    pub fn from_view_projection(matrix: Mat4) -> Self {
        let row = |index: usize| {
            Vec4::new(
                matrix.x_axis[index],
                matrix.y_axis[index],
                matrix.z_axis[index],
                matrix.w_axis[index],
            )
        };
        let (x, y, z, w) = (row(0), row(1), row(2), row(3));
        let mut planes = [Vec4::ZERO; 6];
        let mut count = 0;
        for plane in [w + x, w - x, w + y, w - y, w - z, z] {
            let length = plane.truncate().length();
            // An infinite far plane degenerates to `0 * p + near >= 0`; it culls nothing.
            if length > 1.0e-6 {
                planes[count] = plane / length;
                count += 1;
            }
        }
        Self { planes, count }
    }

    fn planes(&self) -> &[Vec4] {
        &self.planes[..self.count]
    }

    pub fn intersects_sphere(&self, centre: Vec3, radius: f32) -> bool {
        self.planes()
            .iter()
            .all(|plane| plane.truncate().dot(centre) + plane.w >= -radius)
    }

    /// Whether the box may touch the volume; an unbounded box always does.
    pub fn intersects_aabb(&self, aabb: &Aabb) -> bool {
        if !aabb.is_bounded() {
            return true;
        }
        self.planes().iter().all(|plane| {
            let normal = plane.truncate();
            let corner = Vec3::new(
                if normal.x >= 0.0 { aabb.max.x } else { aabb.min.x },
                if normal.y >= 0.0 { aabb.max.y } else { aabb.min.y },
                if normal.z >= 0.0 { aabb.max.z } else { aabb.min.z },
            );
            normal.dot(corner) + plane.w >= 0.0
        })
    }
}

/// The camera as the selection sees it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ViewPoint {
    pub position: Vec3,
    pub frustum: Frustum,
}

fn sphere_hits_box(centre: Vec3, radius: f32, aabb: &Aabb) -> bool {
    if !aabb.is_bounded() {
        return true;
    }
    let nearest = centre.clamp(aabb.min, aabb.max);
    nearest.distance_squared(centre) <= radius * radius
}

// ---- geometry of the tiles of one light ----

/// The six faces of a point light: the direction each looks along and its up vector. The index is the one the shader
/// derives from the largest component of the receiver's offset: +X, -X, +Y, -Y, +Z, -Z.
const FACE_AXES: [(Vec3, Vec3); 6] = [
    (Vec3::X, Vec3::Y),
    (Vec3::NEG_X, Vec3::Y),
    (Vec3::Y, Vec3::NEG_Z),
    (Vec3::NEG_Y, Vec3::Z),
    (Vec3::Z, Vec3::Y),
    (Vec3::NEG_Z, Vec3::Y),
];

fn reverse_z_infinite(tan_half: f32, near: f32) -> Mat4 {
    let scale = 1.0 / tan_half;
    Mat4::from_cols(
        Vec4::new(scale, 0.0, 0.0, 0.0),
        Vec4::new(0.0, scale, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, -1.0),
        Vec4::new(0.0, 0.0, near, 0.0),
    )
}

/// The near plane of a light's maps: far enough not to waste depth on the light's own fixture, near enough to see what
/// stands right next to it.
fn near_plane(range: f32) -> f32 {
    (range * 0.002).clamp(0.02, 0.2)
}

/// Half the field of view of one map, as a tangent: the sampled region plus a guard band of `GUARD_TEXELS` texels.
fn tan_half_fov(shape: LocalShadowShape, tile_size: u32) -> f32 {
    let guard = 1.0 + 2.0 * GUARD_TEXELS / tile_size as f32;
    match shape {
        LocalShadowShape::Point => guard,
        LocalShadowShape::Spot { outer_angle } => outer_angle.min(MAX_SPOT_HALF_ANGLE).tan() * guard,
    }
}

fn spot_up(axis: Vec3) -> Vec3 {
    if axis.y.abs() > 0.99 { Vec3::X } else { Vec3::Y }
}

/// World to clip of one tile of a light: the pass renders with it and the shader samples with it.
pub(crate) fn tile_view_projection(light: &LocalShadowLight, face: usize, tile_size: u32) -> (Mat4, f32) {
    let near = near_plane(light.range);
    let tan_half = tan_half_fov(light.shape, tile_size);
    let (forward, up) = match light.shape {
        LocalShadowShape::Point => FACE_AXES[face.min(5)],
        LocalShadowShape::Spot { .. } => {
            let axis = light.direction.normalize_or_zero();
            (axis, spot_up(axis))
        }
    };
    let view = pathon_api::glam::camera::rh::view::look_to_mat4(light.position, forward, up);
    (reverse_z_infinite(tan_half, near) * view, tan_half)
}

// ---- which lights get maps ----

#[derive(Clone, Copy, Debug)]
pub(crate) struct Selected {
    pub light: LocalShadowLight,
    pub tile_size: u32,
}

/// How much the light is worth a map: its intensity, damped by how small its reach looks from the camera.
fn score_of(light: &LocalShadowLight, camera: Vec3) -> f32 {
    let distance = light.position.distance(camera).max(0.01);
    let reach = (light.range / distance).min(1.0);
    light.intensity * reach * reach
}

fn visible(light: &LocalShadowLight, view: &ViewPoint) -> bool {
    let (centre, radius) = light.bounding_sphere();
    view.frustum.intersects_sphere(centre, radius)
}

fn max_tile_size(light: &LocalShadowLight, settings: &ShadowSettings) -> u32 {
    match light.shape {
        LocalShadowShape::Point => settings.local_point_resolution,
        LocalShadowShape::Spot { .. } => settings.local_spot_resolution,
    }
}

/// Which resolution step (0 is full, each step halves the side) the distance asks for, as a real number.
fn resolution_step(distance: f32, settings: &ShadowSettings) -> f32 {
    (distance / settings.local_reference_distance).max(1.0).log2()
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a step is a small non-negative whole number"
)]
fn step_from(wanted: f32) -> u32 {
    wanted.round().max(0.0) as u32
}

fn tile_size_for_step(light: &LocalShadowLight, step: u32, settings: &ShadowSettings) -> u32 {
    (max_tile_size(light, settings) >> step.min(31)).max(settings.local_min_resolution)
}

/// Tells which candidates get a map and how large; remembers the last choice so that it does not flicker.
#[derive(Debug, Default)]
pub(crate) struct Selector {
    // Light key to the resolution step it had.
    chosen: HashMap<u64, u32>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Selection {
    pub selected: Vec<Selected>,
    pub candidates: usize,
    pub invalid: usize,
    pub out_of_view: usize,
}

impl Selector {
    pub(crate) fn select(
        &mut self,
        lights: &[LocalShadowLight],
        view: &ViewPoint,
        settings: &ShadowSettings,
    ) -> Selection {
        let mut selection = Selection {
            candidates: lights.len(),
            ..Selection::default()
        };
        let mut ranked: Vec<(f32, &LocalShadowLight)> = Vec::new();
        for light in lights {
            if !light.is_usable() {
                selection.invalid += 1;
            } else if !visible(light, view) {
                selection.out_of_view += 1;
            } else {
                let boost = if self.chosen.contains_key(&light.key) {
                    1.0 + settings.local_stickiness
                } else {
                    1.0
                };
                ranked.push((score_of(light, view.position) * boost, light));
            }
        }
        // Strongest first; equal scores keep the order of the keys, so the choice does not depend on the input order.
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.key.cmp(&b.1.key)));
        ranked.truncate(settings.max_local_lights as usize);

        let mut chosen = HashMap::with_capacity(ranked.len());
        for (_, light) in ranked {
            let distance = light.position.distance(view.position);
            let wanted = resolution_step(distance, settings);
            let step = match self.chosen.get(&light.key) {
                // Keep the current step while the distance stays within half a step beyond its band.
                Some(&current) if (wanted - current as f32).abs() < 0.75 => current,
                _ => step_from(wanted),
            };
            chosen.insert(light.key, step);
            selection.selected.push(Selected {
                light: *light,
                tile_size: tile_size_for_step(light, step, settings),
            });
        }
        self.chosen = chosen;
        selection
    }
}

// ---- the plan of a frame ----

/// One map to be rendered or sampled this frame.
#[derive(Clone, Debug)]
pub(crate) struct PlannedTile {
    pub tile: Tile,
    pub view_projection: Mat4,
    /// Tangent of half the field of view; with the tile size it gives the texel size at a distance.
    pub tan_half: f32,
    /// What the pass draws into it, empty for a map that is still valid.
    pub batches: Vec<Batch>,
    pub dirty: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PlannedLight {
    pub light: LocalShadowLight,
    pub near: f32,
    /// Index of the first of its tiles in `LocalPlan::tiles` (one for a spot light, six for a point light).
    pub first_tile: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PlanStats {
    pub candidates: usize,
    pub invalid: usize,
    pub out_of_view: usize,
    pub selected: usize,
    pub dropped_for_space: usize,
    pub rendered_tiles: usize,
    pub cached_tiles: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LocalPlan {
    pub lights: Vec<PlannedLight>,
    pub tiles: Vec<PlannedTile>,
    /// Model matrix of every drawable caster, indexed by `Draw::caster` of the batches of the tiles.
    pub models: Vec<Mat4>,
    pub stats: PlanStats,
}

impl LocalPlan {
    pub(crate) fn dirty_tiles(&self) -> impl Iterator<Item = &PlannedTile> {
        self.tiles.iter().filter(|tile| tile.dirty)
    }
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    tile: Tile,
    signature: u64,
    // Whether the tile holds what `signature` describes (a fresh tile does not).
    valid: bool,
    last_used: u64,
}

/// The atlas and what is in it, from frame to frame.
#[derive(Debug)]
pub(crate) struct LocalPlanner {
    atlas: Atlas,
    slots: HashMap<(u64, u8), Slot>,
    selector: Selector,
}

impl LocalPlanner {
    pub(crate) fn new(atlas_size: u32, min_tile: u32) -> Self {
        Self {
            atlas: Atlas::new(atlas_size, min_tile),
            slots: HashMap::new(),
            selector: Selector::default(),
        }
    }

    /// Whether the atlas has the size and smallest tile these settings ask for.
    pub(crate) fn fits(&self, settings: &ShadowSettings) -> bool {
        self.atlas.size() == settings.local_atlas_size && self.atlas.min_tile() == settings.local_min_resolution
    }

    /// Forgets what the atlas holds: every map is rendered again. For a lost device or a host that knows the content
    /// of its meshes changed.
    pub(crate) fn invalidate(&mut self) {
        for slot in self.slots.values_mut() {
            slot.valid = false;
        }
    }

    pub(crate) fn plan(
        &mut self,
        lights: &[LocalShadowLight],
        casters: &[ShadowCaster],
        view: &ViewPoint,
        settings: &ShadowSettings,
        frame: u64,
    ) -> LocalPlan {
        let selection = self.selector.select(lights, view, settings);
        let mut stats = PlanStats {
            candidates: selection.candidates,
            invalid: selection.invalid,
            out_of_view: selection.out_of_view,
            ..PlanStats::default()
        };
        self.forget_stale(frame);

        let groups = CasterGroups::new(casters);
        let mut plan = LocalPlan::default();
        let mut tiles_left = ShadowSettings::MAX_LOCAL_TILES as usize;
        for selected in &selection.selected {
            let light = selected.light;
            if light.tiles_needed() > tiles_left {
                stats.dropped_for_space += 1;
                continue;
            }
            let Some(tiles) = self.tiles_of(&selected.light, selected.tile_size, settings, frame) else {
                stats.dropped_for_space += 1;
                continue;
            };
            tiles_left -= tiles.len();
            let first_tile = plan.tiles.len();
            for (face, tile) in (0_u8..).zip(tiles) {
                let (view_projection, tan_half) = tile_view_projection(&light, usize::from(face), tile.size);
                let frustum = Frustum::from_view_projection(view_projection);
                let batches = groups.batches(|caster| {
                    sphere_hits_box(light.position, light.range, &caster.bounds)
                        && frustum.intersects_aabb(&caster.bounds)
                });
                let signature = signature_of(tile, view_projection, &batches, &groups.models);
                let slot = self.slots.get_mut(&(light.key, face)).expect("allocated above");
                let dirty = !slot.valid || slot.signature != signature;
                slot.signature = signature;
                slot.valid = true;
                slot.tile = tile;
                stats.rendered_tiles += usize::from(dirty);
                stats.cached_tiles += usize::from(!dirty);
                plan.tiles.push(PlannedTile {
                    tile,
                    view_projection,
                    tan_half,
                    batches: if dirty { batches } else { Vec::new() },
                    dirty,
                });
            }
            plan.lights.push(PlannedLight {
                light,
                near: near_plane(light.range),
                first_tile,
            });
        }
        stats.selected = plan.lights.len();
        plan.stats = stats;
        plan.models = groups.models;
        plan
    }

    // Maps that nobody asked for in a long while give their room back.
    fn forget_stale(&mut self, frame: u64) {
        let stale: Vec<(u64, u8)> = self
            .slots
            .iter()
            .filter(|(_, slot)| frame.saturating_sub(slot.last_used) > RETAIN_FRAMES)
            .map(|(key, _)| *key)
            .collect();
        for key in stale {
            if let Some(slot) = self.slots.remove(&key) {
                self.atlas.release(slot.tile);
            }
        }
    }

    // The tiles of a light, reusing the ones it has when they have the size, else allocating (at a smaller size when
    // there is no room, down to the smallest). `None` when it does not fit at all.
    fn tiles_of(
        &mut self,
        light: &LocalShadowLight,
        wanted: u32,
        settings: &ShadowSettings,
        frame: u64,
    ) -> Option<Vec<Tile>> {
        let count = light.tiles_needed();
        let existing: Vec<Option<Slot>> = light
            .faces()
            .map(|face| self.slots.get(&(light.key, face)).copied())
            .collect();
        if existing
            .iter()
            .all(|slot| slot.is_some_and(|slot| slot.tile.size == wanted))
        {
            for face in light.faces() {
                if let Some(slot) = self.slots.get_mut(&(light.key, face)) {
                    slot.last_used = frame;
                }
            }
            return Some(existing.iter().flatten().map(|slot| slot.tile).collect());
        }
        for face in light.faces() {
            if let Some(slot) = self.slots.remove(&(light.key, face)) {
                self.atlas.release(slot.tile);
            }
        }
        let mut size = wanted;
        loop {
            if let Some(tiles) = self.allocate_all(count, size, light.key, frame) {
                return Some(tiles);
            }
            if self.evict_oldest_unused(frame) {
                continue;
            }
            if size <= settings.local_min_resolution {
                return None;
            }
            size = (size / 2).max(settings.local_min_resolution);
        }
    }

    fn allocate_all(&mut self, count: usize, size: u32, key: u64, frame: u64) -> Option<Vec<Tile>> {
        let mut tiles = Vec::with_capacity(count);
        for _ in 0..count {
            let Some(tile) = self.atlas.allocate(size) else {
                for tile in tiles {
                    self.atlas.release(tile);
                }
                return None;
            };
            tiles.push(tile);
        }
        for (face, tile) in (0_u8..).zip(&tiles) {
            self.slots.insert(
                (key, face),
                Slot {
                    tile: *tile,
                    signature: 0,
                    valid: false,
                    last_used: frame,
                },
            );
        }
        Some(tiles)
    }

    // Frees the map that has gone unused the longest, but only one that is not used this frame.
    fn evict_oldest_unused(&mut self, frame: u64) -> bool {
        let oldest = self
            .slots
            .iter()
            .filter(|(_, slot)| slot.last_used < frame)
            .min_by_key(|((key, face), slot)| (slot.last_used, *key, *face))
            .map(|(key, _)| *key);
        match oldest.and_then(|key| self.slots.remove(&key)) {
            Some(slot) => {
                self.atlas.release(slot.tile);
                true
            }
            None => false,
        }
    }
}

// What a map would contain: where it is, how it looks at the world and which meshes with which matrices it draws.
fn signature_of(tile: Tile, view_projection: Mat4, batches: &[Batch], models: &[Mat4]) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    tile.hash(&mut hasher);
    for value in view_projection.to_cols_array() {
        value.to_bits().hash(&mut hasher);
    }
    for batch in batches {
        (batch.vertex_buffer, batch.index_buffer, batch.vertex_stride).hash(&mut hasher);
        for draw in &batch.draws {
            (draw.index_count, draw.first_index, draw.base_vertex).hash(&mut hasher);
            if let Some(model) = models.get(draw.caster as usize) {
                for value in model.to_cols_array() {
                    value.to_bits().hash(&mut hasher);
                }
            }
        }
    }
    hasher.finish()
}

/// The index the pbr light array gives `light` when lights are packed as `PackedLights` does: directional lights
/// first (and kinds this version does not know), then the others in order.
pub(crate) fn packed_indices<'a>(lights: impl IntoIterator<Item = &'a Light>) -> Vec<u32> {
    let is_local = |light: &Light| {
        matches!(
            light.kind,
            LightKind::Point | LightKind::Spot { .. } | LightKind::Area { .. }
        )
    };
    let lights: Vec<&Light> = lights.into_iter().collect();
    let directional = lights.iter().filter(|light| !is_local(light)).count();
    let (mut next_directional, mut next_local) = (0_u32, u32::try_from(directional).unwrap_or(u32::MAX));
    lights
        .into_iter()
        .map(|light| {
            let counter = if is_local(light) {
                &mut next_local
            } else {
                &mut next_directional
            };
            let index = *counter;
            *counter += 1;
            index
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use pathon_api::Camera;
    use pathon_api::glam::Affine3A;
    use pathon_api::rhi::{BufferDesc, BufferId, BufferUsage, DeviceApi};
    use pathon_rhi::NullDevice;

    use super::*;

    fn face_of(offset: Vec3) -> usize {
        let abs = offset.abs();
        if abs.x >= abs.y && abs.x >= abs.z {
            usize::from(offset.x < 0.0)
        } else if abs.y >= abs.z {
            2 + usize::from(offset.y < 0.0)
        } else {
            4 + usize::from(offset.z < 0.0)
        }
    }

    fn moved(caster: &ShadowCaster, by: Vec3) -> ShadowCaster {
        ShadowCaster {
            model: Affine3A::from_translation(by) * caster.model,
            bounds: if caster.bounds.is_bounded() {
                Aabb::new(caster.bounds.min + by, caster.bounds.max + by)
            } else {
                caster.bounds
            },
            ..*caster
        }
    }

    fn point(key: u64, at: Vec3, intensity: f32) -> LocalShadowLight {
        LocalShadowLight {
            key,
            light_index: u32::try_from(key).expect("small key"),
            shape: LocalShadowShape::Point,
            position: at,
            direction: Vec3::NEG_Y,
            range: 10.0,
            intensity,
        }
    }

    fn spot(key: u64, at: Vec3, towards: Vec3, outer: f32) -> LocalShadowLight {
        LocalShadowLight {
            shape: LocalShadowShape::Spot { outer_angle: outer },
            direction: towards.normalize(),
            ..point(key, at, 1000.0)
        }
    }

    // A camera at the origin looking down -Z.
    fn view() -> ViewPoint {
        let camera = Camera::look_at(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), 1.0, 1.0);
        ViewPoint {
            position: camera.position,
            frustum: Frustum::from_view_projection(camera.view_projection()),
        }
    }

    fn settings(max_lights: u32) -> ShadowSettings {
        ShadowSettings {
            max_local_lights: max_lights,
            local_atlas_size: 4096,
            ..ShadowSettings::DESKTOP
        }
    }

    fn buffers() -> (BufferId, BufferId) {
        let device = NullDevice::new();
        let make = |usage| {
            device
                .create_buffer(&BufferDesc {
                    label: "buffer",
                    size: 64,
                    usage,
                })
                .expect("buffer")
        };
        (make(BufferUsage::VERTEX), make(BufferUsage::INDEX))
    }

    fn caster_at(buffers: (BufferId, BufferId), at: Vec3) -> ShadowCaster {
        ShadowCaster::new(buffers.0, 12, buffers.1, 0..36, 0, Affine3A::from_translation(at))
            .with_bounds(Aabb::new(at - Vec3::splat(0.5), at + Vec3::splat(0.5)))
    }

    #[test]
    fn a_light_outside_the_view_gets_no_map_and_one_inside_does() {
        let mut selector = Selector::default();
        let lights = [
            point(1, Vec3::new(0.0, 0.0, -5.0), 1000.0),
            point(2, Vec3::new(0.0, 0.0, 40.0), 1000.0),
            point(3, Vec3::new(60.0, 0.0, -5.0), 1000.0),
        ];

        let selection = selector.select(&lights, &view(), &settings(8));

        let keys: Vec<u64> = selection.selected.iter().map(|selected| selected.light.key).collect();
        assert_eq!(
            keys,
            [1],
            "the light behind the camera and the one far to the side are culled"
        );
        assert_eq!(selection.out_of_view, 2);
    }

    #[test]
    fn a_spot_light_pointing_away_from_the_view_is_culled_by_its_cone() {
        let mut selector = Selector::default();
        // Behind the camera's near plane, shining further back: sphere of its cone does not reach the frustum.
        let away = spot(1, Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.0, 1.0), 0.4);
        let towards = spot(2, Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.0, -1.0), 0.4);

        let selection = selector.select(&[away, towards], &view(), &settings(8));

        let keys: Vec<u64> = selection.selected.iter().map(|selected| selected.light.key).collect();
        assert_eq!(keys, [2]);
    }

    #[test]
    fn the_strongest_lights_win_when_there_are_too_many() {
        let mut selector = Selector::default();
        let lights: Vec<LocalShadowLight> = (0..6)
            .map(|index| {
                point(
                    index,
                    Vec3::new(0.0, 0.0, -3.0 - index as f32),
                    100.0 * (index + 1) as f32,
                )
            })
            .collect();

        let selection = selector.select(&lights, &view(), &settings(3));

        let keys: Vec<u64> = selection.selected.iter().map(|selected| selected.light.key).collect();
        assert_eq!(keys.len(), 3);
        assert!(keys.contains(&5), "the brightest is chosen: {keys:?}");
        assert!(!keys.contains(&0), "the dimmest is not: {keys:?}");
    }

    #[test]
    fn a_light_that_has_a_map_keeps_it_against_a_slightly_better_challenger() {
        let mut selector = Selector::default();
        let incumbent = point(1, Vec3::new(0.0, 0.0, -5.0), 1000.0);
        let challenger = point(2, Vec3::new(0.0, 0.0, -5.0), 1100.0);
        let settings = settings(1);
        selector.select(&[incumbent], &view(), &settings);

        let selection = selector.select(&[incumbent, challenger], &view(), &settings);

        assert_eq!(selection.selected[0].light.key, 1, "10% more is not worth a swap");
        let mut fresh = Selector::default();
        assert_eq!(
            fresh.select(&[incumbent, challenger], &view(), &settings).selected[0]
                .light
                .key,
            2,
            "without the history the stronger light wins"
        );
        let much_stronger = point(3, Vec3::new(0.0, 0.0, -5.0), 5000.0);
        assert_eq!(
            selector
                .select(&[incumbent, much_stronger], &view(), &settings)
                .selected[0]
                .light
                .key,
            3
        );
    }

    #[test]
    fn the_resolution_halves_with_distance_and_does_not_flicker_at_the_boundary() {
        let settings = settings(8);
        let mut selector = Selector::default();
        let size_at = |selector: &mut Selector, distance: f32| {
            let camera = Camera::look_at(Vec3::new(0.0, 0.0, distance), Vec3::ZERO, 1.0, 1.0);
            let view = ViewPoint {
                position: camera.position,
                frustum: Frustum::from_view_projection(camera.view_projection()),
            };
            selector
                .select(&[spot(1, Vec3::ZERO, Vec3::Z, 0.6)], &view, &settings)
                .selected[0]
                .tile_size
        };

        assert_eq!(size_at(&mut selector, 3.0), 1024, "within the reference distance");
        assert_eq!(size_at(&mut selector, 12.0), 512, "one doubling out");
        assert_eq!(size_at(&mut selector, 100.0), 128, "never below the smallest");
        // Walk back and forth across the boundary between 512 (12 m) and 256 (24 m).
        let mut selector = Selector::default();
        let first = size_at(&mut selector, 12.0);
        let sizes: Vec<u32> = [16.0, 17.0, 16.5, 17.5, 16.0].map(|d| size_at(&mut selector, d)).into();
        assert!(sizes.iter().all(|size| *size == first), "{first} then {sizes:?}");
    }

    #[test]
    fn a_point_light_uses_six_tiles_and_a_spot_light_one() {
        let mut planner = LocalPlanner::new(4096, 128);
        let lights = [
            point(1, Vec3::new(0.0, 0.0, -4.0), 1000.0),
            spot(2, Vec3::new(1.0, 0.0, -4.0), Vec3::Z, 0.5),
        ];

        let plan = planner.plan(&lights, &[], &view(), &settings(8), 1);

        assert_eq!(plan.lights.len(), 2);
        assert_eq!(plan.tiles.len(), 7);
        assert_eq!(plan.lights[0].first_tile, 0);
        assert_eq!(plan.lights[1].first_tile, 6);
        for (index, a) in plan.tiles.iter().enumerate() {
            for b in &plan.tiles[index + 1..] {
                let (a, b) = (a.tile, b.tile);
                assert!(
                    a.x + a.size <= b.x || b.x + b.size <= a.x || a.y + a.size <= b.y || b.y + b.size <= a.y,
                    "{a:?} and {b:?} overlap"
                );
            }
        }
    }

    #[test]
    fn an_unchanged_scene_renders_nothing_the_second_time_and_a_moved_caster_only_what_it_touches() {
        let buffers = buffers();
        let mut planner = LocalPlanner::new(4096, 128);
        let lights = [spot(1, Vec3::new(0.0, 2.0, -3.0), Vec3::new(0.0, -1.0, 0.0), 0.5)];
        let mut casters = vec![
            caster_at(buffers, Vec3::new(0.0, 0.5, -3.0)),
            caster_at(buffers, Vec3::new(20.0, 0.5, -3.0)),
        ];
        let settings = settings(8);

        let first = planner.plan(&lights, &casters, &view(), &settings, 1);
        let second = planner.plan(&lights, &casters, &view(), &settings, 2);
        assert_eq!((first.stats.rendered_tiles, first.stats.cached_tiles), (1, 0));
        assert_eq!(
            (second.stats.rendered_tiles, second.stats.cached_tiles),
            (0, 1),
            "nothing moved, the map is reused"
        );
        assert_eq!(second.tiles[0].batches, []);

        casters[1] = moved(&casters[1], Vec3::new(1.0, 0.0, 0.0));
        let far_moved = planner.plan(&lights, &casters, &view(), &settings, 3);
        assert_eq!(
            far_moved.stats.rendered_tiles, 0,
            "a caster that is outside the light's reach moves and the map stays valid"
        );

        casters[0] = moved(&casters[0], Vec3::new(0.1, 0.0, 0.0));
        let near_moved = planner.plan(&lights, &casters, &view(), &settings, 4);
        assert_eq!(near_moved.stats.rendered_tiles, 1);
        assert_eq!(near_moved.tiles[0].batches[0].draws.len(), 1);

        planner.invalidate();
        assert_eq!(
            planner
                .plan(&lights, &casters, &view(), &settings, 5)
                .stats
                .rendered_tiles,
            1
        );
    }

    #[test]
    fn a_light_that_leaves_the_view_keeps_its_map_for_a_while_and_gets_it_back_valid() {
        let buffers = buffers();
        let mut planner = LocalPlanner::new(4096, 128);
        let light = point(1, Vec3::new(0.0, 0.0, -4.0), 1000.0);
        let casters = [caster_at(buffers, Vec3::new(1.0, 0.0, -4.0))];
        let settings = settings(8);
        let rendered = planner.plan(&[light], &casters, &view(), &settings, 1);
        assert_eq!(rendered.stats.rendered_tiles, 6);

        let gone = planner.plan(
            &[light],
            &casters,
            &ViewPoint {
                position: Vec3::new(0.0, 0.0, 100.0),
                frustum: Frustum::from_view_projection(
                    Camera::look_at(Vec3::new(0.0, 0.0, 100.0), Vec3::new(0.0, 0.0, 200.0), 1.0, 1.0).view_projection(),
                ),
            },
            &settings,
            2,
        );
        assert!(gone.lights.is_empty(), "culled, so no map is rendered or sampled");
        assert_eq!(gone.stats.rendered_tiles, 0);

        let back = planner.plan(&[light], &casters, &view(), &settings, 3);
        assert_eq!(back.stats.cached_tiles, 6, "the content was kept");
        assert_eq!(back.stats.rendered_tiles, 0);
    }

    #[test]
    fn when_the_atlas_is_full_the_strongest_lights_keep_their_size_and_the_last_one_is_dropped() {
        let mut planner = LocalPlanner::new(2048, 128);
        // The spot light takes a quarter of the atlas at 1024, each point light another 6 x 512^2 = three eighths of
        // what is left; the third point light finds nothing.
        let lights = [
            spot(1, Vec3::new(0.0, 0.0, -3.0), Vec3::Z, 0.5),
            point(2, Vec3::new(1.0, 0.0, -3.0), 900.0),
            point(3, Vec3::new(2.0, 0.0, -3.0), 800.0),
            point(4, Vec3::new(-1.0, 0.0, -3.0), 700.0),
        ];

        let plan = planner.plan(&lights, &[], &view(), &settings(8), 1);

        let sizes: Vec<u32> = plan
            .lights
            .iter()
            .map(|planned| plan.tiles[planned.first_tile].tile.size)
            .collect();
        assert_eq!(sizes, [1024, 512, 512]);
        assert_eq!(plan.stats.dropped_for_space, 1);
    }

    #[test]
    fn a_light_that_does_not_fit_at_full_size_gets_a_smaller_map() {
        let mut planner = LocalPlanner::new(1024, 128);

        let plan = planner.plan(
            &[point(1, Vec3::new(0.0, 0.0, -3.0), 900.0)],
            &[],
            &view(),
            &settings(8),
            1,
        );

        assert_eq!(plan.tiles.len(), 6);
        assert!(
            plan.tiles.iter().all(|tile| tile.tile.size == 256),
            "six 512 tiles do not fit in 1024^2"
        );
    }

    #[test]
    fn invalid_lights_are_counted_and_skipped() {
        let mut selector = Selector::default();
        let mut broken = point(1, Vec3::new(0.0, 0.0, -3.0), 1000.0);
        broken.range = f32::NAN;
        let flat_spot = spot(2, Vec3::new(0.0, 0.0, -3.0), Vec3::Z, 0.0);

        let selection = selector.select(&[broken, flat_spot], &view(), &settings(8));

        assert_eq!((selection.invalid, selection.selected.len()), (2, 0));
    }

    #[test]
    fn the_face_that_sees_a_direction_is_the_one_the_shader_picks() {
        for (face, (forward, _)) in FACE_AXES.iter().enumerate() {
            assert_eq!(face_of(*forward * 3.0 + Vec3::new(0.2, 0.1, -0.1) * 0.5), face);
            let light = point(1, Vec3::ZERO, 1.0);
            let (matrix, _) = tile_view_projection(&light, face, 256);
            let clip = matrix * (*forward * 4.0).extend(1.0);
            assert!(clip.w > 0.0 && (clip.x / clip.w).abs() < 1.0e-5 && (clip.y / clip.w).abs() < 1.0e-5);
        }
    }

    #[test]
    fn a_face_sees_a_little_beyond_its_ninety_degrees_by_the_guard_band() {
        let light = point(1, Vec3::ZERO, 1.0);
        let (matrix, tan_half) = tile_view_projection(&light, 4, 256);

        assert!(tan_half > 1.0 && tan_half < 1.1);
        // The edge of the 90 degree face, (1, 0, 1) from the light, is inside the map by the guard band.
        let clip = matrix * Vec4::new(1.0, 0.0, 1.0, 1.0);
        assert!(clip.x.abs() / clip.w < 1.0 && clip.x.abs() / clip.w > 0.9);
    }

    #[test]
    fn packed_indices_follow_the_pbr_light_array() {
        use pathon_api::glam::Affine3A;
        let lights = [
            Light::point(Affine3A::IDENTITY, 800.0),
            Light::directional(Affine3A::IDENTITY, 1.0e5),
            Light::spot(Affine3A::IDENTITY, 800.0, 0.2, 0.5),
            Light::directional(Affine3A::IDENTITY, 5.0e4),
        ];

        assert_eq!(packed_indices(lights.iter()), [2, 0, 3, 1]);
    }

    #[test]
    fn a_spot_cone_sphere_contains_the_apex_and_the_rim() {
        for (half, range) in [(0.2_f32, 10.0_f32), (0.7, 10.0), (1.2, 5.0)] {
            let light = spot(1, Vec3::new(1.0, 2.0, 3.0), Vec3::new(0.3, -1.0, 0.2), half);
            let light = LocalShadowLight { range, ..light };
            let (centre, radius) = light.bounding_sphere();
            let axis = light.direction.normalize();
            let side = axis.any_orthonormal_vector();
            let rim = light.position + axis * range + side * (range * half.tan());

            assert!(light.position.distance(centre) <= radius * 1.0001, "apex");
            assert!(rim.distance(centre) <= radius * 1.0001, "rim of half angle {half}");
        }
    }
}
