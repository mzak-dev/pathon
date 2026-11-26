// The CPU half of a frame of shadows: which casters each cascade draws, grouped by the buffers they live in so the
// depth pass binds once per group.

use pathon_api::glam::Mat4;
use pathon_api::rhi::BufferId;
use pathon_scene::Aabb;

use crate::cascade::{CameraFrustum, Cascade, CascadeRequest, LightBasis, plan_cascades};
use crate::input::ShadowCaster;
use crate::settings::ShadowSettings;

/// One indexed draw of a caster; `caster` is its slot in the model matrix array.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Draw {
    pub index_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
    pub caster: u32,
}

/// The draws of one cascade that share vertex and index buffers.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Batch {
    pub vertex_buffer: BufferId,
    pub vertex_stride: u32,
    pub index_buffer: BufferId,
    pub draws: Vec<Draw>,
}

#[derive(Clone, Debug)]
pub(crate) struct FramePlan {
    pub cascades: Vec<Cascade>,
    pub basis: LightBasis,
    /// Per cascade, its batches.
    pub batches: Vec<Vec<Batch>>,
    /// Model matrix of every caster that some cascade draws, indexed by `Draw::caster`.
    pub models: Vec<Mat4>,
}

impl FramePlan {
    pub(crate) fn draw_count(&self) -> usize {
        self.batches.iter().flatten().map(|batch| batch.draws.len()).sum()
    }
}

type BufferKey = (BufferId, BufferId, u32);

fn key_of(caster: &ShadowCaster) -> BufferKey {
    (caster.vertex_buffer, caster.index_buffer, caster.vertex_stride)
}

pub(crate) fn is_drawable(caster: &ShadowCaster) -> bool {
    caster.index_count > 0 && caster.model.is_finite()
}

/// The drawable casters grouped by the buffers they use (groups in the order they first appear), with a slot in the
/// model matrix array for each, so that a group is a contiguous range of it.
#[derive(Clone, Debug)]
pub(crate) struct CasterGroups<'a> {
    groups: Vec<(BufferKey, Vec<&'a ShadowCaster>)>,
    slots: Vec<Vec<u32>>,
    /// Model matrix of every drawable caster, indexed by `Draw::caster`.
    pub models: Vec<Mat4>,
}

impl<'a> CasterGroups<'a> {
    pub(crate) fn new(casters: &'a [ShadowCaster]) -> Self {
        let mut groups: Vec<(BufferKey, Vec<&ShadowCaster>)> = Vec::new();
        for caster in casters.iter().filter(|caster| is_drawable(caster)) {
            let key = key_of(caster);
            match groups.iter_mut().find(|(candidate, _)| *candidate == key) {
                Some((_, members)) => members.push(caster),
                None => groups.push((key, vec![caster])),
            }
        }
        let mut models = Vec::new();
        let mut slots: Vec<Vec<u32>> = Vec::with_capacity(groups.len());
        for (_, members) in &groups {
            slots.push(
                members
                    .iter()
                    .map(|caster| {
                        models.push(Mat4::from(caster.model));
                        u32::try_from(models.len() - 1).unwrap_or(u32::MAX)
                    })
                    .collect(),
            );
        }
        Self { groups, slots, models }
    }

    /// The draws of the casters `accepts` says yes to, grouped; a group with none is left out.
    pub(crate) fn batches(&self, mut accepts: impl FnMut(&ShadowCaster) -> bool) -> Vec<Batch> {
        self.groups
            .iter()
            .zip(&self.slots)
            .filter_map(|((key, members), slots)| {
                let draws: Vec<Draw> = members
                    .iter()
                    .zip(slots)
                    .filter(|(caster, _)| accepts(caster))
                    .map(|(caster, slot)| Draw {
                        index_count: caster.index_count,
                        first_index: caster.first_index,
                        base_vertex: caster.base_vertex,
                        caster: *slot,
                    })
                    .collect();
                (!draws.is_empty()).then_some(Batch {
                    vertex_buffer: key.0,
                    vertex_stride: key.2,
                    index_buffer: key.1,
                    draws,
                })
            })
            .collect()
    }
}

pub(crate) fn plan_frame(
    casters: &[ShadowCaster],
    frustum: &CameraFrustum,
    basis: LightBasis,
    settings: &ShadowSettings,
) -> FramePlan {
    let boxes: Vec<Aabb> = casters
        .iter()
        .filter(|caster| is_drawable(caster))
        .map(|caster| caster.bounds)
        .collect();
    let cascades = plan_cascades(
        frustum,
        &basis,
        &CascadeRequest {
            cascade_count: settings.cascade_count as usize,
            resolution: settings.resolution,
            max_distance: settings.max_distance,
            scheme: settings.splits,
            fit: settings.fit,
            constant_bias: settings.constant_bias,
            unbounded_caster_distance: settings.unbounded_caster_distance,
            casters: &boxes,
        },
    );
    let groups = CasterGroups::new(casters);
    let batches = cascades
        .iter()
        .map(|cascade| groups.batches(|caster| cascade.may_receive_from(&basis, &caster.bounds)))
        .collect();
    FramePlan {
        cascades,
        basis,
        batches,
        models: groups.models,
    }
}

#[cfg(test)]
mod tests {
    use pathon_api::Camera;
    use pathon_api::glam::{Affine3A, Vec3};
    use pathon_api::rhi::{BufferDesc, BufferUsage, DeviceApi};
    use pathon_rhi::NullDevice;

    use super::*;

    struct Buffers {
        vertices: [BufferId; 2],
        indices: BufferId,
    }

    fn buffers() -> Buffers {
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
        Buffers {
            vertices: [make(BufferUsage::VERTEX), make(BufferUsage::VERTEX)],
            indices: make(BufferUsage::INDEX),
        }
    }

    fn caster(buffers: &Buffers, vertices: usize, at: Vec3) -> ShadowCaster {
        let model = Affine3A::from_translation(at);
        ShadowCaster::new(buffers.vertices[vertices], 12, buffers.indices, 0..36, 0, model)
            .with_bounds(Aabb::new(at - Vec3::ONE, at + Vec3::ONE))
    }

    fn frustum() -> CameraFrustum {
        CameraFrustum::from_camera(&Camera::look_at(
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(0.0, 1.0, -10.0),
            1.0,
            1.5,
        ))
        .expect("frustum")
    }

    fn sun() -> LightBasis {
        LightBasis::new(Vec3::new(-0.3, -1.0, -0.2)).expect("direction")
    }

    #[test]
    fn casters_are_grouped_by_buffers_and_culled_per_cascade() {
        let buffers = buffers();
        let casters = [
            caster(&buffers, 0, Vec3::new(0.0, 0.0, -5.0)),
            caster(&buffers, 1, Vec3::new(1.0, 0.0, -6.0)),
            caster(&buffers, 0, Vec3::new(-1.0, 0.0, -7.0)),
            caster(&buffers, 0, Vec3::new(5000.0, 0.0, 5000.0)),
        ];

        let plan = plan_frame(&casters, &frustum(), sun(), &ShadowSettings::DESKTOP);

        assert_eq!(plan.cascades.len(), 4);
        assert_eq!(plan.models.len(), 4, "every caster has a slot");
        let nearest: Vec<usize> = plan.batches[0].iter().map(|batch| batch.draws.len()).collect();
        assert_eq!(
            nearest,
            [2, 1],
            "two casters share the first buffer, the far one is culled"
        );
        assert!(
            plan.batches
                .iter()
                .flatten()
                .all(|batch| batch.draws.iter().all(|draw| draw.caster != 2)),
            "a caster 7 km away is in no cascade"
        );
        let slots: Vec<u32> = plan.batches[0]
            .iter()
            .flat_map(|batch| batch.draws.iter().map(|d| d.caster))
            .collect();
        assert_eq!(
            slots,
            [0, 1, 3],
            "slots follow the groups so a batch is a contiguous range"
        );
    }

    #[test]
    fn a_caster_without_indices_or_with_a_broken_model_is_not_drawn() {
        let buffers = buffers();
        let mut empty = caster(&buffers, 0, Vec3::new(0.0, 0.0, -5.0));
        empty.index_count = 0;
        let mut broken = caster(&buffers, 0, Vec3::new(0.0, 0.0, -5.0));
        broken.model = Affine3A::from_translation(Vec3::NAN);

        let plan = plan_frame(&[empty, broken], &frustum(), sun(), &ShadowSettings::DESKTOP);

        assert_eq!(plan.draw_count(), 0);
        assert_eq!(plan.models, []);
    }

    #[test]
    fn a_caster_without_bounds_reaches_every_cascade() {
        let buffers = buffers();
        let unbounded = caster(&buffers, 0, Vec3::new(0.0, 0.0, -5.0)).with_bounds(Aabb::UNBOUNDED);

        let plan = plan_frame(&[unbounded], &frustum(), sun(), &ShadowSettings::DESKTOP);

        assert_eq!(plan.draw_count(), 4);
    }
}
