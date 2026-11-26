//! CPU references for the tests: the octahedral mapping, the Fibonacci directions and a Monte Carlo estimate of the
//! light a point of a closed box room receives from the walls.

use pathon_api::glam::Vec3;

fn sign_not_zero(value: f32) -> f32 {
    if value >= 0.0 { 1.0 } else { -1.0 }
}

/// The octahedral mapping of a unit vector to `[-1, 1]^2` (Cigolle et al. 2014), as `gi::common::oct_encode`.
pub fn oct_encode(direction: Vec3) -> [f32; 2] {
    let sum = direction.x.abs() + direction.y.abs() + direction.z.abs();
    let (x, y) = (direction.x / sum, direction.y / sum);
    if direction.z < 0.0 {
        [(1.0 - y.abs()) * sign_not_zero(x), (1.0 - x.abs()) * sign_not_zero(y)]
    } else {
        [x, y]
    }
}

/// The inverse of [`oct_encode`].
pub fn oct_decode(encoded: [f32; 2]) -> Vec3 {
    let (x, y) = (encoded[0], encoded[1]);
    let z = 1.0 - x.abs() - y.abs();
    if z < 0.0 {
        Vec3::new(
            (1.0 - y.abs()) * sign_not_zero(x),
            (1.0 - x.abs()) * sign_not_zero(y),
            z,
        )
        .normalize()
    } else {
        Vec3::new(x, y, z).normalize()
    }
}

/// Spherical Fibonacci point `index` of `count`, as `gi::common::spherical_fibonacci`.
pub fn spherical_fibonacci(index: u32, count: u32) -> Vec3 {
    let phi = std::f32::consts::TAU * (index as f32 * 0.618_034).fract();
    let cos_theta = 1.0 - (2.0 * index as f32 + 1.0) / count as f32;
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    Vec3::new(phi.cos() * sin_theta, phi.sin() * sin_theta, cos_theta)
}

/// A deterministic generator, so a failing estimate can be reproduced from its seed.
#[derive(Clone, Debug)]
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next_f32(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
    }
}

/// A point light (candela), unshadowed: in a convex room nothing stands between it and a wall.
#[derive(Clone, Copy, Debug)]
pub struct PointLight {
    pub position: Vec3,
    pub color_intensity: Vec3,
}

/// An axis-aligned box room with diffuse walls, numbered +x, -x, +y, -y, +z, -z. Lit by point lights and the walls'
/// own emission.
#[derive(Clone, Debug)]
pub struct Room {
    pub min: Vec3,
    pub max: Vec3,
    pub albedo: [Vec3; 6],
    pub emission: [Vec3; 6],
    pub lights: Vec<PointLight>,
}

struct WallHit {
    point: Vec3,
    normal: Vec3,
    wall: usize,
}

impl Room {
    /// A room with the same albedo on every wall and no emission.
    pub fn uniform(min: Vec3, max: Vec3, albedo: Vec3) -> Self {
        Self {
            min,
            max,
            albedo: [albedo; 6],
            emission: [Vec3::ZERO; 6],
            lights: Vec::new(),
        }
    }

    fn hit(&self, origin: Vec3, direction: Vec3) -> WallHit {
        let mut best = (f32::INFINITY, 0_usize);
        for axis in 0..3 {
            let component = direction[axis];
            if component.abs() < 1.0e-9 {
                continue;
            }
            let (wall, plane) = if component > 0.0 {
                (axis * 2, self.max[axis])
            } else {
                (axis * 2 + 1, self.min[axis])
            };
            let t = (plane - origin[axis]) / component;
            if t > 1.0e-5 && t < best.0 {
                best = (t, wall);
            }
        }
        let mut normal = Vec3::ZERO;
        normal[best.1 / 2] = if best.1 % 2 == 0 { -1.0 } else { 1.0 };
        WallHit {
            point: origin + direction * best.0,
            normal,
            wall: best.1,
        }
    }

    fn direct(&self, point: Vec3, normal: Vec3) -> Vec3 {
        self.lights
            .iter()
            .map(|light| {
                let to_light = light.position - point;
                let distance_square = to_light.length_squared();
                let cosine = normal.dot(to_light / distance_square.sqrt()).max(0.0);
                light.color_intensity * (cosine / distance_square)
            })
            .sum()
    }

    // Radiance leaving the wall point `hit` towards the room, with `depth` more bounces allowed.
    fn leaving(&self, hit: &WallHit, depth: u32, rng: &mut Lcg) -> Vec3 {
        let albedo = self.albedo[hit.wall];
        let mut radiance = self.emission[hit.wall] + albedo * self.direct(hit.point, hit.normal) / std::f32::consts::PI;
        if depth > 0 {
            let direction = cosine_direction(hit.normal, rng);
            let next = self.hit(hit.point, direction);
            radiance += albedo * self.leaving(&next, depth - 1, rng);
        }
        radiance
    }

    /// The cosine-weighted mean radiance (`E / pi`) of the light that arrives at `point` from the walls, with up to
    /// `bounces` bounces on them, from `samples` paths. Direct light at `point` itself is not part of it.
    pub fn mean_cosine_radiance(&self, point: Vec3, normal: Vec3, samples: u32, bounces: u32, seed: u64) -> Vec3 {
        let mut rng = Lcg(seed);
        let mut sum = Vec3::ZERO;
        for _ in 0..samples {
            let direction = cosine_direction(normal, &mut rng);
            let hit = self.hit(point, direction);
            sum += self.leaving(&hit, bounces.saturating_sub(1), &mut rng);
        }
        sum / samples as f32
    }
}

fn cosine_direction(normal: Vec3, rng: &mut Lcg) -> Vec3 {
    let (first, second) = (rng.next_f32(), rng.next_f32());
    let radius = first.sqrt();
    let angle = std::f32::consts::TAU * second;
    let (across, along) = (radius * angle.cos(), radius * angle.sin());
    let up = (1.0 - first).max(0.0).sqrt();
    let helper = if normal.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
    let tangent = normal.cross(helper).normalize();
    let bitangent = normal.cross(tangent);
    tangent * across + bitangent * along + normal * up
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octahedral_mapping_round_trips() {
        for index in 0..256 {
            let direction = spherical_fibonacci(index, 256);
            let back = oct_decode(oct_encode(direction));
            assert!((back - direction).length() < 1.0e-4, "{direction} -> {back}");
        }
    }

    #[test]
    fn fibonacci_points_are_unit_vectors_spread_over_the_sphere() {
        let points: Vec<Vec3> = (0..128).map(|index| spherical_fibonacci(index, 128)).collect();

        assert!(points.iter().all(|point| (point.length() - 1.0).abs() < 1.0e-4));
        let mean: Vec3 = points.iter().sum::<Vec3>() / 128.0;
        assert!(mean.length() < 0.02);
    }

    #[test]
    fn a_closed_room_of_emitting_walls_reaches_the_series_limit() {
        // Walls of albedo a that all emit e: the radiance everywhere is e / (1 - a).
        let mut room = Room::uniform(Vec3::splat(-1.0), Vec3::splat(1.0), Vec3::splat(0.5));
        room.emission = [Vec3::ONE; 6];

        let value = room.mean_cosine_radiance(Vec3::ZERO, Vec3::Y, 20_000, 24, 1);

        assert!((value.x - 2.0).abs() < 0.04, "{value}");
    }
}
