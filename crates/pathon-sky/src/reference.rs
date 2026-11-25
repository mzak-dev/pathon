//! The sky on the CPU, in `f64`: the oracle of the GPU tests and the source of the environment picture for
//! image-based lighting (ADR-0045). The model is Hillaire 2020: a transmittance table, a multiple-scattering table and
//! a ray march; the GPU replaces the march by a sky-view table. Radiances are per unit sun illuminance unless a
//! function says nits.

use std::f64::consts::PI;
use std::ops::Range;
use std::sync::Mutex;

use pathon_api::TaskPool;
use pathon_ibl::{EquirectImage, IblError};

use crate::Atmosphere;
use crate::atmosphere::{OZONE_CENTER_KM, OZONE_HALF_WIDTH_KM};

pub type Rgb = [f64; 3];

/// Size of the transmittance table, `(width over the cosine of the zenith angle, height over the radius)`.
pub const TRANSMITTANCE_TABLE: (usize, usize) = (256, 64);
const TRANSMITTANCE_STEPS: u32 = 40;
const MULTISCATTER_SIZE: usize = 16;
const MULTISCATTER_SIDE: usize = 8;
const MULTISCATTER_STEPS: u32 = 16;
/// Steps of the march that produces the environment picture.
pub const ENVIRONMENT_STEPS: u32 = 24;

pub fn add(a: Rgb, b: Rgb) -> Rgb {
    std::array::from_fn(|channel| a[channel] + b[channel])
}

pub fn scale(a: Rgb, factor: f64) -> Rgb {
    a.map(|value| value * factor)
}

pub fn multiply(a: Rgb, b: Rgb) -> Rgb {
    std::array::from_fn(|channel| a[channel] * b[channel])
}

pub fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn normalize(a: [f64; 3]) -> [f64; 3] {
    let length = dot(a, a).sqrt();
    a.map(|value| value / length)
}

pub fn rayleigh_phase(cosine: f64) -> f64 {
    3.0 / (16.0 * PI) * (1.0 + cosine * cosine)
}

/// Cornette-Shanks, the Mie phase of Hillaire's paper; `cosine` is between the view ray and the direction to the sun.
pub fn mie_phase(anisotropy: f64, cosine: f64) -> f64 {
    let g2 = anisotropy * anisotropy;
    let k = 3.0 / (8.0 * PI) * (1.0 - g2) / (2.0 + g2);
    k * (1.0 + cosine * cosine) / (1.0 + g2 - 2.0 * anisotropy * cosine).max(1e-4).powf(1.5)
}

struct Medium {
    rayleigh: Rgb,
    mie: Rgb,
    extinction: Rgb,
}

impl Medium {
    fn scattering(&self) -> Rgb {
        add(self.rayleigh, self.mie)
    }
}

/// The geometry and the media of an atmosphere: everything that needs no table.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Planet {
    pub bottom_radius: f64,
    pub top_radius: f64,
    rayleigh: Rgb,
    rayleigh_inverse_height: f64,
    mie_scattering: Rgb,
    mie_extinction: Rgb,
    mie_inverse_height: f64,
    pub mie_anisotropy: f64,
    ozone: Rgb,
}

impl Planet {
    pub fn new(atmosphere: &Atmosphere) -> Self {
        let widen = |values: [f32; 3]| values.map(f64::from);
        Self {
            bottom_radius: f64::from(atmosphere.bottom_radius_km),
            top_radius: f64::from(atmosphere.top_radius_km),
            rayleigh: widen(atmosphere.rayleigh_scattering),
            rayleigh_inverse_height: 1.0 / f64::from(atmosphere.rayleigh_scale_height_km),
            mie_scattering: widen(atmosphere.mie_scattering),
            mie_extinction: widen(atmosphere.mie_extinction),
            mie_inverse_height: 1.0 / f64::from(atmosphere.mie_scale_height_km),
            mie_anisotropy: f64::from(atmosphere.mie_anisotropy),
            ozone: widen(atmosphere.ozone_absorption),
        }
    }

    fn medium(&self, altitude: f64) -> Medium {
        let height = altitude.max(0.0);
        let rayleigh_density = (-height * self.rayleigh_inverse_height).exp();
        let mie_density = (-height * self.mie_inverse_height).exp();
        let ozone_density = (1.0 - (height - OZONE_CENTER_KM).abs() / OZONE_HALF_WIDTH_KM).max(0.0);
        let rayleigh = scale(self.rayleigh, rayleigh_density);
        let mie = scale(self.mie_scattering, mie_density);
        let extinction = std::array::from_fn(|channel| {
            rayleigh[channel] + self.mie_extinction[channel] * mie_density + self.ozone[channel] * ozone_density
        });
        Medium {
            rayleigh,
            mie,
            extinction,
        }
    }

    // `r^2 - bottom^2` as a product, which keeps its precision for a camera a few metres above the ground.
    fn rho_squared(&self, radius: f64) -> f64 {
        ((radius - self.bottom_radius) * (radius + self.bottom_radius)).max(0.0)
    }

    pub fn horizon_cosine(&self, radius: f64) -> f64 {
        -self.rho_squared(radius).sqrt() / radius
    }

    /// Distance from `radius` along `mu` (cosine of the zenith angle) to the top of the atmosphere.
    pub fn top_distance(&self, radius: f64, mu: f64) -> f64 {
        let discriminant = radius * radius * (mu * mu - 1.0) + self.top_radius * self.top_radius;
        (-radius * mu + discriminant.max(0.0).sqrt()).max(0.0)
    }

    /// Distance to the ground, `None` when the ray flies off into space.
    pub fn ground_distance(&self, radius: f64, mu: f64) -> Option<f64> {
        let discriminant = radius * radius * mu * mu - self.rho_squared(radius);
        (mu < 0.0 && discriminant >= 0.0).then(|| -radius * mu - discriminant.sqrt())
    }

    pub fn hits_ground(&self, radius: f64, mu: f64) -> bool {
        mu < self.horizon_cosine(radius)
    }

    /// The point `t` km from the camera along `direction`: radius, altitude and the up vector of the planet there.
    fn point_along(&self, camera_radius: f64, direction: [f64; 3], t: f64) -> (f64, f64, [f64; 3]) {
        let position = [direction[0] * t, camera_radius + direction[1] * t, direction[2] * t];
        let radius = dot(position, position).sqrt();
        let altitude = (self.rho_squared(camera_radius) + 2.0 * camera_radius * direction[1] * t + t * t)
            / (radius + self.bottom_radius);
        (radius, altitude, position.map(|value| value / radius))
    }

    /// Optical depth to the top of the atmosphere integrated step by step: the reference of the transmittance table.
    pub fn transmittance_integral(&self, radius: f64, mu: f64, steps: u32) -> Rgb {
        let distance = self.top_distance(radius, mu);
        let step = distance / f64::from(steps);
        let direction = [(1.0 - mu * mu).max(0.0).sqrt(), mu, 0.0];
        let mut depth = [0.0; 3];
        for index in 0..steps {
            let t = (f64::from(index) + 0.5) * step;
            let (_, altitude, _) = self.point_along(radius, direction, t);
            depth = add(depth, scale(self.medium(altitude).extinction, step));
        }
        depth.map(|value| (-value).exp())
    }

    /// Texture coordinates of a point and direction in the transmittance table (Bruneton's parametrisation).
    pub fn transmittance_uv(&self, radius: f64, mu: f64) -> [f64; 2] {
        let horizon = (self.top_radius * self.top_radius - self.bottom_radius * self.bottom_radius).sqrt();
        let rho = self.rho_squared(radius).sqrt();
        let distance = self.top_distance(radius, mu);
        let d_min = self.top_radius - radius;
        let d_max = rho + horizon;
        [
            ((distance - d_min) / (d_max - d_min)).clamp(0.0, 1.0),
            (rho / horizon).clamp(0.0, 1.0),
        ]
    }

    /// Inverse of [`Self::transmittance_uv`]: `(radius, mu)` of a texel.
    pub fn transmittance_params(&self, u: f64, v: f64) -> (f64, f64) {
        let horizon = (self.top_radius * self.top_radius - self.bottom_radius * self.bottom_radius).sqrt();
        let rho = horizon * v;
        let radius = (rho * rho + self.bottom_radius * self.bottom_radius).sqrt();
        let d_min = self.top_radius - radius;
        let d_max = rho + horizon;
        let distance = d_min + u * (d_max - d_min);
        let mu = if distance <= 0.0 {
            1.0
        } else {
            (horizon * horizon - rho * rho - distance * distance) / (2.0 * radius * distance)
        };
        (radius, mu.clamp(-1.0, 1.0))
    }
}

/// The state of the sky one radiance is asked in.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SkyConditions {
    /// Distance of the camera from the planet's centre, km.
    pub camera_radius_km: f64,
    /// Unit vector towards the sun, Y up.
    pub sun_direction: [f64; 3],
    /// Illuminance above the atmosphere per channel, lux.
    pub sun_illuminance: Rgb,
    pub cloud_coverage: f64,
}

/// A table with texel centres at `(i + 0.5) / size`, sampled bilinearly like a texture with a clamping sampler.
struct Table {
    width: usize,
    height: usize,
    texels: Vec<Rgb>,
}

impl Table {
    fn sample(&self, u: f64, v: f64) -> Rgb {
        let x = (u * self.width as f64 - 0.5).clamp(0.0, (self.width - 1) as f64);
        let y = (v * self.height as f64 - 0.5).clamp(0.0, (self.height - 1) as f64);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "clamped to the table, so the floor is a valid index"
        )]
        #[expect(clippy::cast_sign_loss, reason = "clamped to be non-negative")]
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (x - x0 as f64, y - y0 as f64);
        let at = |x: usize, y: usize| self.texels[y * self.width + x];
        let top = add(scale(at(x0, y0), 1.0 - fx), scale(at(x1, y0), fx));
        let bottom = add(scale(at(x0, y1), 1.0 - fx), scale(at(x1, y1), fx));
        add(scale(top, 1.0 - fy), scale(bottom, fy))
    }
}

/// Coordinates of a texture whose first and last texel centres are the ends of the unit range (no extrapolation).
pub fn unit_to_sub_uv(unit: f64, size: usize) -> f64 {
    (unit * (size - 1) as f64 + 0.5) / size as f64
}

/// A planet with its transmittance and multiple-scattering tables: enough to evaluate the sky in any direction.
pub struct SkyModel {
    planet: Planet,
    ground_albedo: Rgb,
    transmittance: Table,
    multiscatter: Table,
}

impl std::fmt::Debug for SkyModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkyModel")
            .field("planet", &self.planet)
            .field("ground_albedo", &self.ground_albedo)
            .finish_non_exhaustive()
    }
}

impl SkyModel {
    pub fn new(atmosphere: &Atmosphere, ground_albedo: [f32; 3], pool: Option<&dyn TaskPool>) -> Self {
        let planet = Planet::new(atmosphere);
        let (width, height) = TRANSMITTANCE_TABLE;
        let mut texels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let (radius, mu) =
                    planet.transmittance_params((x as f64 + 0.5) / width as f64, (y as f64 + 0.5) / height as f64);
                texels.push(planet.transmittance_integral(radius, mu, TRANSMITTANCE_STEPS));
            }
        }
        let mut model = Self {
            planet,
            ground_albedo: ground_albedo.map(f64::from),
            transmittance: Table { width, height, texels },
            multiscatter: Table {
                width: MULTISCATTER_SIZE,
                height: MULTISCATTER_SIZE,
                texels: Vec::new(),
            },
        };
        model.multiscatter.texels = model.multiscatter_texels(pool);
        model
    }

    pub fn planet(&self) -> &Planet {
        &self.planet
    }

    /// Transmittance to the top of the atmosphere from the table (the ray must not hit the ground).
    pub fn transmittance(&self, radius: f64, mu: f64) -> Rgb {
        let [u, v] = self.planet.transmittance_uv(radius, mu);
        self.transmittance.sample(u, v)
    }

    /// Transmittance towards the sun, zero where the planet hides it.
    pub fn sun_transmittance(&self, radius: f64, mu_sun: f64) -> Rgb {
        if self.planet.hits_ground(radius, mu_sun) {
            [0.0; 3]
        } else {
            self.transmittance(radius, mu_sun)
        }
    }

    fn multiscatter_lookup(&self, radius: f64, mu_sun: f64) -> Rgb {
        let u = (mu_sun * 0.5 + 0.5).clamp(0.0, 1.0);
        let v = ((radius - self.planet.bottom_radius) / (self.planet.top_radius - self.planet.bottom_radius))
            .clamp(0.0, 1.0);
        self.multiscatter.sample(
            unit_to_sub_uv(u, MULTISCATTER_SIZE),
            unit_to_sub_uv(v, MULTISCATTER_SIZE),
        )
    }

    fn multiscatter_texels(&self, pool: Option<&dyn TaskPool>) -> Vec<Rgb> {
        let rows = Mutex::new(Vec::with_capacity(MULTISCATTER_SIZE));
        let compute = |range: Range<usize>| {
            for y in range {
                let row: Vec<Rgb> = (0..MULTISCATTER_SIZE)
                    .map(|x| {
                        let unit = |index: usize| index as f64 / (MULTISCATTER_SIZE - 1) as f64;
                        let radius =
                            self.planet.bottom_radius + unit(y) * (self.planet.top_radius - self.planet.bottom_radius);
                        self.multiscatter_texel(radius, unit(x) * 2.0 - 1.0)
                    })
                    .collect();
                rows.lock().expect("rows mutex").push((y, row));
            }
        };
        match pool {
            Some(pool) => pool.parallel_for(MULTISCATTER_SIZE, &compute),
            None => compute(0..MULTISCATTER_SIZE),
        }
        let mut rows = rows.into_inner().expect("rows mutex");
        rows.sort_by_key(|(y, _)| *y);
        rows.into_iter().flat_map(|(_, row)| row).collect()
    }

    // Psi_ms of the paper: second-order scattering with isotropic phase, summed over the higher orders.
    fn multiscatter_texel(&self, radius: f64, mu_sun: f64) -> Rgb {
        let planet = &self.planet;
        let sun = [(1.0 - mu_sun * mu_sun).max(0.0).sqrt(), mu_sun, 0.0];
        let side = MULTISCATTER_SIDE;
        let (mut luminance, mut transfer) = ([0.0; 3], [0.0; 3]);
        for i in 0..side {
            for j in 0..side {
                let theta = 2.0 * PI * (i as f64 + 0.5) / side as f64;
                let cosine = 1.0 - 2.0 * (j as f64 + 0.5) / side as f64;
                let sine = (1.0 - cosine * cosine).sqrt();
                let direction = [theta.cos() * sine, cosine, theta.sin() * sine];
                let ground = planet.ground_distance(radius, direction[1]);
                let t_max = ground.unwrap_or_else(|| planet.top_distance(radius, direction[1]));
                let step = t_max / f64::from(MULTISCATTER_STEPS);
                let mut throughput = [1.0; 3];
                for index in 0..MULTISCATTER_STEPS {
                    let t = (f64::from(index) + 0.5) * step;
                    let (point_radius, altitude, up) = planet.point_along(radius, direction, t);
                    let medium = planet.medium(altitude);
                    let step_transmittance = medium.extinction.map(|value| (-value * step).exp());
                    let scattering = medium.scattering();
                    let sun_light = self.sun_transmittance(point_radius, dot(up, sun));
                    let source = scale(multiply(sun_light, scattering), 1.0 / (4.0 * PI));
                    luminance = add(
                        luminance,
                        multiply(
                            throughput,
                            integrate_step(source, medium.extinction, step_transmittance, step),
                        ),
                    );
                    transfer = add(
                        transfer,
                        multiply(
                            throughput,
                            integrate_step(scattering, medium.extinction, step_transmittance, step),
                        ),
                    );
                    throughput = multiply(throughput, step_transmittance);
                }
                if let Some(distance) = ground {
                    luminance = add(
                        luminance,
                        multiply(throughput, self.ground_light(radius, direction, distance, sun)),
                    );
                }
            }
        }
        let samples = (side * side) as f64;
        let (luminance, transfer) = (scale(luminance, 1.0 / samples), scale(transfer, 1.0 / samples));
        std::array::from_fn(|channel| luminance[channel] / (1.0 - transfer[channel]))
    }

    // Light the sun leaves on the ground at `distance` along `direction`, reflected as a Lambertian surface.
    fn ground_light(&self, radius: f64, direction: [f64; 3], distance: f64, sun: [f64; 3]) -> Rgb {
        let (point_radius, _, up) = self.planet.point_along(radius, direction, distance);
        let cosine = dot(up, sun);
        scale(
            multiply(self.sun_transmittance(point_radius, cosine), self.ground_albedo),
            cosine.max(0.0) / PI,
        )
    }

    /// Luminance scattered towards the camera along `direction` per unit sun illuminance, without the sun disc; a ray
    /// that meets the planet adds the light of the ground. `steps` are spaced quadratically: dense near the camera.
    pub fn inscattered(&self, camera_radius: f64, direction: [f64; 3], sun: [f64; 3], steps: u32) -> Rgb {
        let planet = &self.planet;
        let ground = planet.ground_distance(camera_radius, direction[1]);
        let t_max = ground.unwrap_or_else(|| planet.top_distance(camera_radius, direction[1]));
        let view_sun = dot(direction, sun);
        let (rayleigh_phase, mie_phase) = (rayleigh_phase(view_sun), mie_phase(planet.mie_anisotropy, view_sun));
        let boundary = |index: u32| t_max * (f64::from(index) / f64::from(steps)).powi(2);
        let (mut luminance, mut throughput) = ([0.0; 3], [1.0; 3]);
        for index in 0..steps {
            let (start, end) = (boundary(index), boundary(index + 1));
            let (step, t) = (end - start, f64::midpoint(start, end));
            let (point_radius, altitude, up) = planet.point_along(camera_radius, direction, t);
            let medium = planet.medium(altitude);
            let step_transmittance = medium.extinction.map(|value| (-value * step).exp());
            let mu_sun = dot(up, sun);
            let sun_light = self.sun_transmittance(point_radius, mu_sun);
            let multiscatter = self.multiscatter_lookup(point_radius, mu_sun);
            let source = std::array::from_fn(|channel| {
                sun_light[channel] * (medium.rayleigh[channel] * rayleigh_phase + medium.mie[channel] * mie_phase)
                    + multiscatter[channel] * medium.scattering()[channel]
            });
            luminance = add(
                luminance,
                multiply(
                    throughput,
                    integrate_step(source, medium.extinction, step_transmittance, step),
                ),
            );
            throughput = multiply(throughput, step_transmittance);
        }
        if let Some(distance) = ground {
            luminance = add(
                luminance,
                multiply(throughput, self.ground_light(camera_radius, direction, distance, sun)),
            );
        }
        luminance
    }

    /// Zenith radiance of the overcast sky (CIE: `L(elevation) = Lz (1 + 2 sin) / 3`) that has a quarter of the sun's
    /// horizontal illuminance (ADR-0045).
    pub fn overcast_zenith_radiance(&self, conditions: &SkyConditions) -> Rgb {
        overcast_zenith_radiance(&self.planet, conditions)
    }

    /// Radiance in nits towards `direction`, sun disc excluded, clouds included.
    pub fn radiance(&self, conditions: &SkyConditions, direction: [f64; 3], steps: u32) -> Rgb {
        let clear = multiply(
            self.inscattered(conditions.camera_radius_km, direction, conditions.sun_direction, steps),
            conditions.sun_illuminance,
        );
        blend_overcast(
            clear,
            self.overcast_zenith_radiance(conditions),
            conditions.cloud_coverage,
            direction[1],
        )
    }

    /// The sky as an equirectangular picture (nits), for [`pathon_ibl::IblSource`]. Rows go to the task pool.
    pub fn environment_image(
        &self,
        conditions: &SkyConditions,
        width: u32,
        pool: Option<&dyn TaskPool>,
    ) -> Result<EquirectImage, IblError> {
        let (columns, rows) = (width as usize, (width / 2) as usize);
        let overcast = self.overcast_zenith_radiance(conditions);
        let finished = Mutex::new(Vec::with_capacity(rows));
        let compute = |range: Range<usize>| {
            for y in range {
                let mut row = Vec::with_capacity(columns * 3);
                for x in 0..columns {
                    let direction = pathon_ibl::reference::equirect_direction(
                        (x as f64 + 0.5) / columns as f64,
                        (y as f64 + 0.5) / rows as f64,
                    );
                    let clear = multiply(
                        self.inscattered(
                            conditions.camera_radius_km,
                            direction,
                            conditions.sun_direction,
                            ENVIRONMENT_STEPS,
                        ),
                        conditions.sun_illuminance,
                    );
                    let radiance = blend_overcast(clear, overcast, conditions.cloud_coverage, direction[1]);
                    #[expect(clippy::cast_possible_truncation, reason = "radiance is stored as f32")]
                    row.extend(radiance.map(|value| value as f32));
                }
                finished.lock().expect("rows mutex").push((y, row));
            }
        };
        match pool {
            Some(pool) => pool.parallel_for(rows, &compute),
            None => compute(0..rows),
        }
        let mut finished = finished.into_inner().expect("rows mutex");
        finished.sort_by_key(|(y, _)| *y);
        let pixels: Vec<f32> = finished.into_iter().flat_map(|(_, row)| row).collect();
        #[expect(clippy::cast_possible_truncation, reason = "the picture is at most 1024 wide")]
        EquirectImage::from_rgb_f32(columns as u32, rows as u32, &pixels)
    }
}

/// Zenith radiance of the overcast sky without a model: it only needs the transmittance of the sun's ray.
pub fn overcast_zenith_radiance(planet: &Planet, conditions: &SkyConditions) -> Rgb {
    let mu_sun = conditions.sun_direction[1];
    if mu_sun <= 0.0 || planet.hits_ground(conditions.camera_radius_km, mu_sun) {
        return [0.0; 3];
    }
    let transmittance = planet.transmittance_integral(conditions.camera_radius_km, mu_sun, 64);
    let horizontal = scale(multiply(conditions.sun_illuminance, transmittance), 0.25 * mu_sun);
    scale(horizontal, 9.0 / (7.0 * PI))
}

pub fn blend_overcast(clear: Rgb, overcast_zenith: Rgb, coverage: f64, direction_y: f64) -> Rgb {
    let shape = (1.0 + 2.0 * direction_y.max(0.0)) / 3.0;
    std::array::from_fn(|channel| clear[channel] * (1.0 - coverage) + overcast_zenith[channel] * shape * coverage)
}

/// Illuminance of a surface facing the sun at the camera, before clouds (lux per channel).
pub fn sun_illuminance_at_camera(planet: &Planet, conditions: &SkyConditions) -> Rgb {
    let mu_sun = conditions.sun_direction[1];
    if planet.hits_ground(conditions.camera_radius_km, mu_sun) {
        return [0.0; 3];
    }
    multiply(
        conditions.sun_illuminance,
        planet.transmittance_integral(conditions.camera_radius_km, mu_sun, 64),
    )
}

// Closed form of `integral of S * T(t) dt` over a step in which the source `S` is constant (Hillaire 2020, eq. 7).
fn integrate_step(source: Rgb, extinction: Rgb, step_transmittance: Rgb, step: f64) -> Rgb {
    std::array::from_fn(|channel| {
        if extinction[channel] > 1e-12 {
            (source[channel] - source[channel] * step_transmittance[channel]) / extinction[channel]
        } else {
            source[channel] * step
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn earth() -> Planet {
        Planet::new(&Atmosphere::EARTH)
    }

    #[test]
    fn the_transmittance_parametrisation_round_trips() {
        let planet = earth();
        for (u, v) in [(0.1, 0.1), (0.5, 0.5), (0.9, 0.3), (0.25, 0.95)] {
            let (radius, mu) = planet.transmittance_params(u, v);
            let [back_u, back_v] = planet.transmittance_uv(radius, mu);
            assert!(
                (back_u - u).abs() < 1e-6 && (back_v - v).abs() < 1e-6,
                "{u} {v} -> {back_u} {back_v}"
            );
        }
    }

    #[test]
    fn the_sun_at_the_zenith_loses_little_and_the_horizon_loses_much() {
        let planet = earth();
        let zenith = planet.transmittance_integral(6360.001, 1.0, 256);
        let horizon = planet.transmittance_integral(6360.001, 0.0, 256);
        assert!(
            zenith[0] > 0.85 && zenith[2] > 0.6 && zenith[0] > zenith[2],
            "{zenith:?}"
        );
        assert!(horizon[2] < 0.05 && horizon[0] > horizon[2] * 5.0, "{horizon:?}");
    }

    #[test]
    fn a_ray_up_does_not_meet_the_ground_and_a_ray_down_does() {
        let planet = earth();
        assert!(planet.ground_distance(6360.5, 0.3).is_none());
        let down = planet.ground_distance(6360.5, -0.5).expect("ground");
        assert!((down - 1.0).abs() < 0.1, "{down}");
        assert!(planet.hits_ground(6360.5, -0.02));
        assert!(!planet.hits_ground(6360.5, 0.0));
    }

    #[test]
    fn the_phase_functions_integrate_to_one() {
        let sphere = |phase: &dyn Fn(f64) -> f64| {
            let steps = 20_000;
            (0..steps)
                .map(|index| {
                    let cosine = -1.0 + (f64::from(index) + 0.5) * 2.0 / f64::from(steps);
                    phase(cosine) * 2.0 * PI * 2.0 / f64::from(steps)
                })
                .sum::<f64>()
        };
        assert!((sphere(&rayleigh_phase) - 1.0).abs() < 1e-3);
        assert!((sphere(&|cosine| mie_phase(0.8, cosine)) - 1.0).abs() < 1e-2);
    }
}
