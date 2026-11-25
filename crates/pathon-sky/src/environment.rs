use std::sync::Arc;

use pathon_api::glam::Vec3;
use pathon_api::graph::Stage;
use pathon_api::rhi::RhiError;
use pathon_api::{Diagnostic, Negotiation, RecordCtx, RenderFeature, SetupCtx, TaskPool};
use pathon_ibl::{IblSettings, IblSource};

use crate::frame::{SkyFrame, resolve_settings};
use crate::reference::SkyModel;
use crate::{Atmosphere, SkyIblSettings};

/// What the environment picture depends on. Two keys that differ by less than the thresholds give the same lighting
/// for practical purposes, so the second does not trigger a rebuild.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct EnvironmentKey {
    sun: Vec3,
    illuminance: [f32; 3],
    cloud_coverage: f32,
    ground_albedo: [f32; 3],
    altitude_km: f32,
    atmosphere: Atmosphere,
    image_width: u32,
    ibl: IblSettings,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Change {
    None,
    Gradual,
    // The picture has another shape, so waiting for the interval would show the old one.
    Structural,
}

impl EnvironmentKey {
    pub(crate) fn from_frame(frame: &SkyFrame) -> Self {
        let settings = &frame.settings;
        #[expect(clippy::cast_possible_truncation, reason = "the altitude only decides a threshold")]
        let altitude_km = (frame.conditions.camera_radius_km - f64::from(settings.atmosphere.bottom_radius_km)) as f32;
        Self {
            sun: settings.sun_direction,
            illuminance: std::array::from_fn(|channel| settings.sun_illuminance_lux * settings.sun_tint[channel]),
            cloud_coverage: settings.cloud_coverage,
            ground_albedo: settings.ground_albedo,
            altitude_km,
            atmosphere: settings.atmosphere,
            image_width: settings.ibl.image_width,
            ibl: settings.ibl.ibl,
        }
    }

    fn change_from(&self, built: &Self, thresholds: &SkyIblSettings) -> Change {
        if self.atmosphere != built.atmosphere || self.image_width != built.image_width || self.ibl != built.ibl {
            return Change::Structural;
        }
        let sun_angle = self
            .sun
            .cross(built.sun)
            .length()
            .atan2(self.sun.dot(built.sun))
            .to_degrees();
        let relative = |now: f32, before: f32| (now - before).abs() / before.abs().max(1e-3);
        let illuminance = (0..3)
            .map(|channel| relative(self.illuminance[channel], built.illuminance[channel]))
            .fold(0.0, f32::max);
        let albedo = (0..3)
            .map(|channel| (self.ground_albedo[channel] - built.ground_albedo[channel]).abs())
            .fold(0.0, f32::max);
        let cloud = (self.cloud_coverage - built.cloud_coverage).abs();
        let altitude_meters = (self.altitude_km - built.altitude_km).abs() * 1000.0;
        let gradual = sun_angle > thresholds.min_sun_angle_degrees
            || illuminance > thresholds.min_relative_change
            || albedo > thresholds.min_relative_change
            || cloud > thresholds.min_relative_change
            || altitude_meters > thresholds.min_altitude_change_meters;
        if gradual { Change::Gradual } else { Change::None }
    }
}

/// Decides when the environment is built again: on the first frame, on a structural change, and after a change above
/// the thresholds once the interval since the last build has passed.
#[derive(Debug, Default)]
pub(crate) struct EnvironmentTracker {
    built: Option<(EnvironmentKey, f64)>,
}

impl EnvironmentTracker {
    pub(crate) fn should_rebuild(&self, key: &EnvironmentKey, now_seconds: f64, thresholds: &SkyIblSettings) -> bool {
        let Some((built, at)) = &self.built else {
            return true;
        };
        match key.change_from(built, thresholds) {
            Change::None => false,
            Change::Structural => true,
            // A clock that went backwards counts as an elapsed interval.
            Change::Gradual => {
                let waited = now_seconds - at;
                !(0.0..f64::from(thresholds.min_interval_seconds)).contains(&waited)
            }
        }
    }

    pub(crate) fn mark_built(&mut self, key: EnvironmentKey, now_seconds: f64) {
        self.built = Some((key, now_seconds));
    }

    fn forget(&mut self) {
        self.built = None;
    }
}

/// Publishes `SunLight` every frame and, when `SkyIblSettings::enabled`, the sky as an `IblSource` after a change
/// above the thresholds. Runs before `IblPack` in the same frame, so lighting never lags the sky by one.
pub struct SkyEnvironmentFeature {
    task_pool: Option<Arc<dyn TaskPool>>,
    model: Option<(Atmosphere, [f32; 3], SkyModel)>,
    tracker: EnvironmentTracker,
    reported_settings: bool,
    reported_altitude: bool,
}

impl std::fmt::Debug for SkyEnvironmentFeature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkyEnvironmentFeature")
            .field("tracker", &self.tracker)
            .finish_non_exhaustive()
    }
}

impl Default for SkyEnvironmentFeature {
    fn default() -> Self {
        Self::new()
    }
}

impl SkyEnvironmentFeature {
    pub fn new() -> Self {
        Self {
            task_pool: None,
            model: None,
            tracker: EnvironmentTracker::default(),
            reported_settings: false,
            reported_altitude: false,
        }
    }

    // The tables of the CPU model depend on the atmosphere and the ground only, so a moving sun reuses them.
    fn model(&mut self, atmosphere: &Atmosphere, albedo: [f32; 3]) -> &SkyModel {
        let current = self
            .model
            .as_ref()
            .is_some_and(|(cached_atmosphere, cached_albedo, _)| {
                cached_atmosphere == atmosphere && cached_albedo.map(f32::to_bits) == albedo.map(f32::to_bits)
            });
        if !current {
            let model = SkyModel::new(atmosphere, albedo, self.task_pool.as_deref());
            self.model = Some((*atmosphere, albedo, model));
        }
        &self.model.as_ref().expect("the model was just stored").2
    }

    fn report_problems(&mut self, ctx: &mut RecordCtx<'_>, problems: &[String]) {
        if problems.is_empty() {
            self.reported_settings = false;
        } else if !self.reported_settings {
            self.reported_settings = true;
            ctx.report(
                Diagnostic::warning("A0001", format!("sky settings repaired: {}", problems.join("; ")))
                    .with_hint("keep SkySettings inside the documented ranges"),
            );
        }
    }
}

impl RenderFeature for SkyEnvironmentFeature {
    fn name(&self) -> &'static str {
        "sky environment"
    }

    fn stage(&self) -> Stage {
        Stage::Setup
    }

    fn before(&self) -> &[&'static str] {
        &["ibl"]
    }

    fn negotiate(&mut self, _caps: &pathon_api::rhi::Caps) -> Negotiation {
        Negotiation::Full
    }

    fn setup(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        self.task_pool = Some(ctx.task_pool_handle());
        self.tracker.forget();
        Ok(())
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let (settings, problems) = resolve_settings(ctx.shared());
        self.report_problems(ctx, &problems);
        let view = ctx.view();
        let (camera, now) = (view.camera, view.time_seconds);
        let altitude = settings.camera_altitude_meters(camera.position.y);
        let limit = settings.atmosphere.thickness_km() * 1000.0;
        if (0.0..=limit).contains(&altitude) {
            self.reported_altitude = false;
        } else if !self.reported_altitude {
            self.reported_altitude = true;
            ctx.report(
                Diagnostic::warning(
                    "A0003",
                    format!("camera height {altitude} m is outside the atmosphere (0..={limit} m), clamped"),
                )
                .with_hint("the sky is drawn for a camera inside the atmosphere; see CameraHeight"),
            );
        }

        let frame = SkyFrame::new(&settings, &camera);
        ctx.shared_mut().insert(frame.sun_light);
        if !settings.ibl.enabled {
            return Ok(());
        }
        let key = EnvironmentKey::from_frame(&frame);
        if !self.tracker.should_rebuild(&key, now, &settings.ibl) {
            return Ok(());
        }
        // Whatever the outcome, the same key is not retried every frame.
        self.tracker.mark_built(key, now);
        let pool = self.task_pool.clone();
        let image = self
            .model(&settings.atmosphere, settings.ground_albedo)
            .environment_image(&frame.conditions, settings.ibl.image_width, pool.as_deref());
        match image {
            Ok(image) => {
                ctx.shared_mut()
                    .insert(IblSource::new(Arc::new(image), settings.ibl.ibl));
            }
            Err(error) => ctx.report(
                Diagnostic::error(
                    "A0002",
                    format!("the sky could not become an environment picture: {error}"),
                )
                .with_hint("this is a bug of the sky, not of the settings"),
            ),
        }
        Ok(())
    }

    fn on_device_reset(&mut self, ctx: &mut SetupCtx<'_>) -> Result<(), RhiError> {
        self.setup(ctx)
    }
}

#[cfg(test)]
mod tests {
    use pathon_api::Camera;

    use super::*;
    use crate::SkySettings;

    fn key(settings: &SkySettings) -> EnvironmentKey {
        EnvironmentKey::from_frame(&SkyFrame::new(&settings.sanitized().0, &Camera::default()))
    }

    fn thresholds() -> SkyIblSettings {
        SkyIblSettings {
            min_interval_seconds: 1.0,
            ..SkyIblSettings::default()
        }
    }

    fn built_at(settings: &SkySettings, time: f64) -> EnvironmentTracker {
        let mut tracker = EnvironmentTracker::default();
        tracker.mark_built(key(settings), time);
        tracker
    }

    #[test]
    fn the_first_frame_builds_and_an_unchanged_sky_does_not() {
        let settings = SkySettings::default();
        assert!(EnvironmentTracker::default().should_rebuild(&key(&settings), 0.0, &thresholds()));
        assert!(!built_at(&settings, 0.0).should_rebuild(&key(&settings), 100.0, &thresholds()));
    }

    #[test]
    fn a_sun_that_moves_a_little_does_not_rebuild_and_one_that_moves_a_lot_waits_for_the_interval() {
        let settings = SkySettings::default().with_sun(40.0, 10.0);
        let tracker = built_at(&settings, 10.0);
        let nudged = SkySettings::default().with_sun(40.5, 10.0);
        assert!(!tracker.should_rebuild(&key(&nudged), 99.0, &thresholds()));
        let moved = SkySettings::default().with_sun(43.0, 10.0);
        assert!(
            !tracker.should_rebuild(&key(&moved), 10.5, &thresholds()),
            "inside the interval"
        );
        assert!(
            tracker.should_rebuild(&key(&moved), 11.0, &thresholds()),
            "after the interval"
        );
    }

    #[test]
    fn clouds_albedo_brightness_and_height_each_cross_their_threshold() {
        let settings = SkySettings::default();
        let tracker = built_at(&settings, 0.0);
        let changed = |settings: SkySettings| tracker.should_rebuild(&key(&settings), 5.0, &thresholds());
        assert!(changed(SkySettings {
            cloud_coverage: 0.2,
            ..settings.clone()
        }));
        assert!(changed(SkySettings {
            ground_albedo: [0.6; 3],
            ..settings.clone()
        }));
        assert!(changed(SkySettings {
            sun_illuminance_lux: 60_000.0,
            ..settings.clone()
        }));
        assert!(!changed(SkySettings {
            sun_illuminance_lux: 121_000.0,
            ..settings.clone()
        }));
    }

    #[test]
    fn a_new_atmosphere_or_picture_size_rebuilds_at_once() {
        let settings = SkySettings::default();
        let tracker = built_at(&settings, 10.0);
        let mut other = settings.clone();
        other.atmosphere.mie_anisotropy = 0.6;
        assert!(tracker.should_rebuild(&key(&other), 10.0, &thresholds()));
        let mut wider = settings;
        wider.ibl.image_width = 256;
        assert!(tracker.should_rebuild(&key(&wider), 10.0, &thresholds()));
    }

    #[test]
    fn a_clock_that_runs_backwards_does_not_hold_a_rebuild_back() {
        let settings = SkySettings::default().with_sun(40.0, 0.0);
        let tracker = built_at(&settings, 50.0);
        let moved = SkySettings::default().with_sun(60.0, 0.0);
        assert!(tracker.should_rebuild(&key(&moved), 1.0, &thresholds()));
    }
}
