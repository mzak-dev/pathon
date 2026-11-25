use std::f32::consts::PI;

use bytemuck::{Pod, Zeroable};
use pathon_api::glam::Mat4;

use crate::SkyQuality;
use crate::frame::SkyFrame;

/// The uniform block of every sky shader (`SkyParams` in `atmosphere.wgsl`): all `vec4` after the matrix, so the layout
/// is the same under every packing rule.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct SkyUniform {
    inverse_view_projection: [[f32; 4]; 4],
    rayleigh: [f32; 4],
    mie_scattering: [f32; 4],
    mie_extinction: [f32; 4],
    ozone_absorption: [f32; 4],
    ground_albedo: [f32; 4],
    sun_direction: [f32; 4],
    sun_illuminance: [f32; 4],
    overcast: [f32; 4],
    radii: [f32; 4],
    sizes: [f32; 4],
    view_sizes: [f32; 4],
}

const _: () = assert!(size_of::<SkyUniform>() == 64 + 11 * 16);

fn with_w(xyz: [f32; 3], w: f32) -> [f32; 4] {
    [xyz[0], xyz[1], xyz[2], w]
}

#[expect(
    clippy::cast_precision_loss,
    reason = "sizes of tables and render targets are far below 2^24"
)]
fn float(value: u32) -> f32 {
    value as f32
}

impl SkyUniform {
    pub(crate) fn new(
        frame: &SkyFrame,
        quality: &SkyQuality,
        inverse_view_projection: Mat4,
        render_size: (u32, u32),
    ) -> Self {
        let settings = &frame.settings;
        let atmosphere = &settings.atmosphere;
        let sun = settings.sun_direction;
        let solid_angle = 4.0 * PI * (0.5 * settings.sun_angular_radius).sin().powi(2);
        let illuminance = std::array::from_fn(|channel| settings.sun_illuminance_lux * settings.sun_tint[channel]);
        let disc_cap = if settings.draw_sun_disc {
            settings.sun_disc_max_radiance
        } else {
            0.0
        };
        #[expect(clippy::cast_possible_truncation, reason = "the GPU works in f32")]
        let camera_radius = frame.conditions.camera_radius_km as f32;
        Self {
            inverse_view_projection: inverse_view_projection.to_cols_array_2d(),
            rayleigh: with_w(
                atmosphere.rayleigh_scattering,
                atmosphere.rayleigh_scale_height_km.recip(),
            ),
            mie_scattering: with_w(atmosphere.mie_scattering, atmosphere.mie_scale_height_km.recip()),
            mie_extinction: with_w(atmosphere.mie_extinction, atmosphere.mie_anisotropy),
            ozone_absorption: with_w(atmosphere.ozone_absorption, 0.0),
            ground_albedo: with_w(settings.ground_albedo, 0.0),
            sun_direction: [sun.x, sun.y, sun.z, settings.sun_angular_radius],
            sun_illuminance: with_w(illuminance, solid_angle),
            overcast: with_w(frame.overcast_zenith, settings.cloud_coverage),
            radii: [
                atmosphere.bottom_radius_km,
                atmosphere.top_radius_km,
                camera_radius,
                disc_cap,
            ],
            sizes: [
                float(quality.transmittance_size.0),
                float(quality.transmittance_size.1),
                float(quality.multiscatter_size),
                0.0,
            ],
            view_sizes: [
                float(quality.sky_view_size.0),
                float(quality.sky_view_size.1),
                float(render_size.0),
                float(render_size.1),
            ],
        }
    }
}
