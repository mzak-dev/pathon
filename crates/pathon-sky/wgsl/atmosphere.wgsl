const PI: f32 = 3.14159265358979;
const OZONE_CENTER_KM: f32 = 25.0;
const OZONE_HALF_WIDTH_KM: f32 = 15.0;

struct SkyParams {
    inverse_view_projection: mat4x4<f32>,
    // xyz: scattering (1/km), w: 1 / scale height (1/km)
    rayleigh: vec4<f32>,
    mie_scattering: vec4<f32>,
    // xyz: extinction, w: anisotropy
    mie_extinction: vec4<f32>,
    ozone_absorption: vec4<f32>,
    ground_albedo: vec4<f32>,
    // xyz: unit vector to the sun, w: angular radius (rad)
    sun_direction: vec4<f32>,
    // xyz: illuminance above the atmosphere (lux), w: solid angle of the disc (sr)
    sun_illuminance: vec4<f32>,
    // xyz: zenith radiance of the overcast sky (nits), w: cloud coverage
    overcast: vec4<f32>,
    // x: bottom radius, y: top radius, z: camera radius (km), w: cap of the disc radiance (nits)
    radii: vec4<f32>,
    // xy: transmittance table size, z: multiple-scattering table size
    sizes: vec4<f32>,
    // xy: sky-view table size, zw: render size in pixels
    view_sizes: vec4<f32>,
}

@group(0) @binding(0) var<uniform> sky: SkyParams;

struct Medium {
    rayleigh: vec3<f32>,
    mie: vec3<f32>,
    extinction: vec3<f32>,
}

fn bottom_radius() -> f32 { return sky.radii.x; }
fn top_radius() -> f32 { return sky.radii.y; }

fn medium_at(altitude: f32) -> Medium {
    let height = max(altitude, 0.0);
    let rayleigh_density = exp(-height * sky.rayleigh.w);
    let mie_density = exp(-height * sky.mie_scattering.w);
    let ozone_density = max(0.0, 1.0 - abs(height - OZONE_CENTER_KM) / OZONE_HALF_WIDTH_KM);
    var medium: Medium;
    medium.rayleigh = sky.rayleigh.xyz * rayleigh_density;
    medium.mie = sky.mie_scattering.xyz * mie_density;
    medium.extinction = medium.rayleigh + sky.mie_extinction.xyz * mie_density + sky.ozone_absorption.xyz * ozone_density;
    return medium;
}

fn rayleigh_phase(cosine: f32) -> f32 {
    return 3.0 / (16.0 * PI) * (1.0 + cosine * cosine);
}

// Cornette-Shanks; `cosine` is between the view ray and the direction to the sun.
fn mie_phase(cosine: f32) -> f32 {
    let g = sky.mie_extinction.w;
    let g2 = g * g;
    let k = 3.0 / (8.0 * PI) * (1.0 - g2) / (2.0 + g2);
    return k * (1.0 + cosine * cosine) / pow(max(1.0 + g2 - 2.0 * g * cosine, 1e-4), 1.5);
}

// r^2 - bottom^2 as a product keeps its precision for a camera a few metres above the ground.
fn rho_squared(radius: f32) -> f32 {
    return max((radius - bottom_radius()) * (radius + bottom_radius()), 0.0);
}

fn horizon_cosine(radius: f32) -> f32 {
    return -sqrt(rho_squared(radius)) / radius;
}

fn top_distance(radius: f32, mu: f32) -> f32 {
    let discriminant = radius * radius * (mu * mu - 1.0) + top_radius() * top_radius();
    return max(-radius * mu + sqrt(max(discriminant, 0.0)), 0.0);
}

// Negative when the ray flies off into space.
fn ground_distance(radius: f32, mu: f32) -> f32 {
    let discriminant = radius * radius * mu * mu - rho_squared(radius);
    if (mu >= 0.0 || discriminant < 0.0) {
        return -1.0;
    }
    return -radius * mu - sqrt(discriminant);
}

struct SamplePoint {
    radius: f32,
    altitude: f32,
    up: vec3<f32>,
}

// The point `t` km from the camera at `camera_radius` along `direction`; the planet's centre is below the camera.
fn point_along(camera_radius: f32, direction: vec3<f32>, t: f32) -> SamplePoint {
    let position = vec3<f32>(direction.x * t, camera_radius + direction.y * t, direction.z * t);
    var point: SamplePoint;
    point.radius = length(position);
    point.altitude = (rho_squared(camera_radius) + 2.0 * camera_radius * direction.y * t + t * t)
        / (point.radius + bottom_radius());
    point.up = position / point.radius;
    return point;
}

// Closed form of the integral of S * T over a step where the source S is constant (Hillaire 2020, eq. 7).
fn integrate_step(source: vec3<f32>, extinction: vec3<f32>, step_transmittance: vec3<f32>, step: f32) -> vec3<f32> {
    let safe = max(extinction, vec3<f32>(1e-9));
    return select(source * step, (source - source * step_transmittance) / safe, extinction > vec3<f32>(1e-9));
}

// Texture coordinates whose first and last texel centres are the ends of the unit range.
fn unit_to_sub_uv(unit: f32, size: f32) -> f32 {
    return (unit * (size - 1.0) + 0.5) / size;
}

fn sub_uv_to_unit(uv: f32, size: f32) -> f32 {
    return (uv - 0.5 / size) * size / (size - 1.0);
}

// Bruneton's parametrisation of (radius, mu) for the transmittance table.
fn transmittance_uv(radius: f32, mu: f32) -> vec2<f32> {
    let horizon = sqrt(top_radius() * top_radius() - bottom_radius() * bottom_radius());
    let rho = sqrt(rho_squared(radius));
    let distance = top_distance(radius, mu);
    let d_min = top_radius() - radius;
    let d_max = rho + horizon;
    return vec2<f32>(clamp((distance - d_min) / (d_max - d_min), 0.0, 1.0), clamp(rho / horizon, 0.0, 1.0));
}

struct RadiusAndMu {
    radius: f32,
    mu: f32,
}

fn transmittance_params(uv: vec2<f32>) -> RadiusAndMu {
    let horizon = sqrt(top_radius() * top_radius() - bottom_radius() * bottom_radius());
    let rho = horizon * uv.y;
    let radius = sqrt(rho * rho + bottom_radius() * bottom_radius());
    let d_min = top_radius() - radius;
    let d_max = rho + horizon;
    let distance = d_min + uv.x * (d_max - d_min);
    var result: RadiusAndMu;
    result.radius = radius;
    result.mu = select(clamp((horizon * horizon - rho * rho - distance * distance) / (2.0 * radius * distance), -1.0, 1.0), 1.0, distance <= 0.0);
    return result;
}
