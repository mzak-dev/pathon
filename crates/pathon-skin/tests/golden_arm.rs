#![cfg(feature = "gpu-tests")]
// The deformed buffers drawn as vertex buffers: an arm at three elbow angles, normals as colour. The golden image is
// what a raster pass sees, so a wrong normal or a wrong bank shows up as a visible difference.

mod support;

use pathon_api::rhi::RhiError;
use pathon_core::RenderError;
use pathon_golden::{Tolerance, announce_adapter, assert_golden, gpu_or_skip};
use pathon_skin::MotionHistory;
use support::{Pose, Rig, backend_from_environment};

#[test]
fn the_skinned_arm_matches_its_golden_image() {
    let test = "the_skinned_arm_matches_its_golden_image";
    let built = Rig::build(backend_from_environment(), 3, 1);
    let Some(mut rig) = gpu_or_skip(test, built, |error| {
        matches!(error, RenderError::Rhi(RhiError::NoAdapter(_)))
    }) else {
        return;
    };
    announce_adapter(test, rig.device().caps());
    let poses: Vec<Pose> = [0.0, 0.7, 1.4]
        .into_iter()
        .map(|angle| Pose::elbow(angle, MotionHistory::Fresh))
        .collect();
    rig.frame(&poses);
    assert_golden("skin_arm", &rig.draw_instances(), Tolerance::cross_backend());
}
