use pathon_api::glam::{Affine3A, Mat4};
use pathon_api::resources::{DeferredRelease, GpuArray};
use pathon_api::rhi::{BufferId, DeviceApi, RhiError};

// Affine transform as three rows of four floats (the last row of a Mat4 is dropped). In WGSL this is `mat3x4<f32>`
// whose columns are these rows; `vec3(dot(r0, p), dot(r1, p), dot(r2, p))` or `p * m` transforms point `p` (w = 1).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct AffineRows(pub [[f32; 4]; 3]);

impl AffineRows {
    pub const IDENTITY: Self = Self([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]);

    pub fn from_mat4(matrix: Mat4) -> Self {
        Self([
            matrix.row(0).to_array(),
            matrix.row(1).to_array(),
            matrix.row(2).to_array(),
        ])
    }
}

impl From<Affine3A> for AffineRows {
    fn from(affine: Affine3A) -> Self {
        Self::from_mat4(Mat4::from(affine))
    }
}

// What a vertex shader needs to write a motion vector: this frame's and last frame's model matrix (ADR-0027).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ObjectTransforms {
    pub model: AffineRows,
    pub previous_model: AffineRows,
}

const _: () = assert!(size_of::<ObjectTransforms>() == 96);

// Object transforms with the previous frame's value next to the current one, as one storage buffer.
#[derive(Debug)]
pub struct TransformHistory {
    objects: GpuArray<ObjectTransforms>,
    moved_since_begin: Vec<usize>,
    last_frame: Option<u64>,
}

impl TransformHistory {
    pub fn new(label: &'static str) -> Self {
        Self {
            objects: GpuArray::new(label),
            moved_since_begin: Vec::new(),
            last_frame: None,
        }
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&ObjectTransforms> {
        self.objects.get(index)
    }

    // A new object has no motion in the frame it appears.
    pub fn push(&mut self, model: AffineRows) -> usize {
        self.objects.push(ObjectTransforms {
            model,
            previous_model: model,
        })
    }

    pub fn set(&mut self, index: usize, model: AffineRows) -> bool {
        let Some(current) = self.objects.get(index).copied() else {
            return false;
        };
        self.moved_since_begin.push(index);
        self.objects.set(
            index,
            ObjectTransforms {
                model,
                previous_model: current.previous_model,
            },
        )
    }

    // Call once per frame before `set`: objects that moved in the previous frame take their current model as
    // the previous one. Only those objects are touched, so a mostly static scene costs next to nothing.
    pub fn begin_frame(&mut self, frame: u64) {
        if self.last_frame == Some(frame) {
            return;
        }
        self.last_frame = Some(frame);
        for index in std::mem::take(&mut self.moved_since_begin) {
            let Some(object) = self.objects.get(index).copied() else {
                continue;
            };
            if object.previous_model != object.model {
                self.objects.set(
                    index,
                    ObjectTransforms {
                        model: object.model,
                        previous_model: object.model,
                    },
                );
            }
        }
    }

    pub fn clear(&mut self) {
        self.objects.clear();
        self.moved_since_begin.clear();
    }

    pub fn sync(
        &mut self,
        device: &dyn DeviceApi,
        release: &mut DeferredRelease,
        frame: u64,
    ) -> Result<BufferId, RhiError> {
        self.objects.sync(device, release, frame)
    }

    pub fn forget_gpu_buffer(&mut self) {
        self.objects.forget_gpu_buffer();
    }
}

#[cfg(test)]
mod tests {
    use pathon_api::glam::Vec3;

    use super::*;

    fn translation(x: f32) -> AffineRows {
        AffineRows::from_mat4(Mat4::from_translation(Vec3::new(x, 0.0, 0.0)))
    }

    #[test]
    fn rows_of_a_translation_hold_it_in_the_fourth_column() {
        let rows = AffineRows::from_mat4(Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)));

        assert_eq!(
            rows.0,
            [[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 2.0], [0.0, 0.0, 1.0, 3.0]]
        );
        assert_eq!(AffineRows::from(Affine3A::IDENTITY), AffineRows::IDENTITY);
    }

    #[test]
    fn a_new_object_has_no_motion() {
        let mut history = TransformHistory::new("transforms");
        let index = history.push(translation(5.0));

        let object = history.get(index).expect("pushed");
        assert_eq!(object.model, object.previous_model);
    }

    #[test]
    fn the_previous_model_is_last_frames_model() {
        let mut history = TransformHistory::new("transforms");
        let index = history.push(translation(0.0));

        history.begin_frame(1);
        history.set(index, translation(1.0));
        let first = *history.get(index).expect("object");
        assert_eq!(
            (first.model, first.previous_model),
            (translation(1.0), translation(0.0))
        );

        history.begin_frame(2);
        history.set(index, translation(3.0));
        let second = *history.get(index).expect("object");
        assert_eq!(
            (second.model, second.previous_model),
            (translation(3.0), translation(1.0))
        );
    }

    #[test]
    fn an_object_that_stops_moving_ends_with_zero_motion() {
        let mut history = TransformHistory::new("transforms");
        let index = history.push(translation(0.0));
        history.begin_frame(1);
        history.set(index, translation(1.0));

        history.begin_frame(2);

        let object = history.get(index).expect("object");
        assert_eq!(
            (object.model, object.previous_model),
            (translation(1.0), translation(1.0))
        );
    }

    #[test]
    fn begin_frame_is_idempotent_for_one_frame_number() {
        let mut history = TransformHistory::new("transforms");
        let index = history.push(translation(0.0));
        history.begin_frame(1);
        history.set(index, translation(1.0));
        history.begin_frame(2);
        history.begin_frame(2);
        history.set(index, translation(2.0));
        history.begin_frame(2);

        let object = history.get(index).expect("object");
        assert_eq!(
            (object.model, object.previous_model),
            (translation(2.0), translation(1.0))
        );
    }

    #[test]
    fn setting_a_missing_object_fails() {
        let mut history = TransformHistory::new("transforms");

        assert!(!history.set(0, AffineRows::IDENTITY));
    }
}
