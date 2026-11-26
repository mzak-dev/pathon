// Rectangular area lights (ADR-0048). The light is a Lambertian emitter of constant luminance, so what a surface point
// receives is that luminance times an integral over the solid angle the rectangle covers.
// Diffuse: the cosine-weighted solid angle of a spherical polygon is 1/2 sum theta_i (gamma_i . n) over its edges after
// clipping to the horizon of n (Arvo 1995, "Applications of Irradiance Tensors to the Simulation of Non-Lambertian
// Phenomena"): exact, no table.
// Specular: Linearly Transformed Cosines (Heitz, Dupuy, Hill and Neubelt 2016): the GGX lobe is a clamped cosine under
// a 3x3 transform M that depends on roughness and view angle; the integral of the lobe over the rectangle is the
// cosine integral of the rectangle transformed by M^-1. The table of M^-1 is our own fit (tools/pathon-ltc-fit).
import package::material::standard::{COAT_F0, MIN_N_DOT_V, front_diffuse, fresnel_schlick};
import package::material::surface::{BsdfResult, Surface};
import package::pbr::lights::{GpuLight, square_falloff_attenuation};

const PI: f32 = 3.14159265358979;

// One node of the LTC table: the inverse transform (a, b, c, d: x' = a x + b z, z' = c x + d z, y' = y) and the integrals
// of the lobe times 1 - F5 (what f0 multiplies) and times F5 (what f90 multiplies), F5 = (1 - v.h)^5.
struct LtcEntry {
    inverse: vec4<f32>,
    scales: vec2<f32>,
}

// theta * gamma of the edge from a to b, both unit vectors: the angle between them times the unit normal of their plane.
fn edge_vector(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    let c = cross(a, b);
    let sine = length(c);
    if (sine < 1.0e-12) {
        return vec3<f32>(0.0);
    }
    return c * (atan2(sine, dot(a, b)) / sine);
}

// Integral of (omega . axis) over the part of the polygon above the horizon of `axis`. The corners are relative to the
// shaded point. A convex quad cut by a plane has at most five corners.
fn cosine_integral(corners: array<vec3<f32>, 4>, axis: vec3<f32>) -> f32 {
    var polygon = corners;
    var clipped: array<vec3<f32>, 6>;
    var count = 0u;
    for (var index = 0u; index < 4u; index++) {
        let current = polygon[index];
        let next = polygon[(index + 1u) % 4u];
        let height_current = dot(current, axis);
        let height_next = dot(next, axis);
        if (height_current >= 0.0 && count < 5u) {
            clipped[count] = current;
            count++;
        }
        if ((height_current >= 0.0) != (height_next >= 0.0) && count < 5u) {
            clipped[count] = current + (next - current) * (height_current / (height_current - height_next));
            count++;
        }
    }
    if (count < 3u) {
        return 0.0;
    }
    for (var index = 0u; index < count; index++) {
        clipped[index] = normalize(clipped[index]);
    }
    var sum = vec3<f32>(0.0);
    for (var index = 0u; index < count; index++) {
        sum += edge_vector(clipped[index], clipped[(index + 1u) % count]);
    }
    return abs(0.5 * dot(sum, axis));
}

// Integral of the GGX lobe (normalised to 1 over the sphere) over the rectangle.
fn lobe_integral(corners: array<vec3<f32>, 4>, normal: vec3<f32>, view: vec3<f32>, entry: LtcEntry) -> f32 {
    let projected = view - normal * dot(view, normal);
    var tangent: vec3<f32>;
    if (dot(projected, projected) > 1.0e-8) {
        tangent = normalize(projected);
    } else {
        tangent = normalize(cross(normal, select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(normal.x) > 0.9)));
    }
    let bitangent = cross(normal, tangent);
    var relative = corners;
    var transformed: array<vec3<f32>, 4>;
    for (var index = 0u; index < 4u; index++) {
        let local = vec3<f32>(dot(relative[index], tangent), dot(relative[index], bitangent), dot(relative[index], normal));
        transformed[index] = vec3<f32>(
            entry.inverse.x * local.x + entry.inverse.y * local.z,
            local.y,
            entry.inverse.z * local.x + entry.inverse.w * local.z,
        );
    }
    return cosine_integral(transformed, vec3<f32>(0.0, 0.0, 1.0)) / PI;
}

fn above_horizon(corners: array<vec3<f32>, 4>, normal: vec3<f32>) -> bool {
    return dot(corners[0], normal) > 0.0 || dot(corners[1], normal) > 0.0 || dot(corners[2], normal) > 0.0
        || dot(corners[3], normal) > 0.0;
}

// Radiance the area light adds towards `view`, split into the diffuse and the specular lobes, like `standard::evaluate`
// does for one direction. `base` and `coat` are the LTC nodes of the base and clear coat lobes.
fn area_evaluate(surface: Surface, view: vec3<f32>, light: GpuLight, base: LtcEntry, coat: LtcEntry) -> BsdfResult {
    var result: BsdfResult;
    let to_center = light.position_range.xyz - surface.position;
    let two_sided = light.area_half_width.w != 0.0;
    if (!two_sided && dot(-to_center, light.direction_kind.xyz) <= 0.0) {
        return result;
    }
    let distance_square = dot(to_center, to_center);
    let window = square_falloff_attenuation(distance_square, light.spot.z);
    if (window <= 0.0) {
        return result;
    }
    let radiance = light.color_intensity.rgb * window;
    let half_width = light.area_half_width.xyz;
    let half_height = light.area_half_height.xyz;
    let corners = array<vec3<f32>, 4>(
        to_center - half_width - half_height,
        to_center + half_width - half_height,
        to_center + half_width + half_height,
        to_center - half_width + half_height,
    );
    let n = surface.normal;
    let n_dot_v = max(abs(dot(n, view)), MIN_N_DOT_V);

    // The fresnel of the diffuse term and of the coat attenuation is taken towards the centre of the light.
    let centre = to_center * inverseSqrt(max(distance_square, 1.0e-8));
    let half_sum = centre + view;
    let half_length_squared = dot(half_sum, half_sum);
    let h = select(n, half_sum * inverseSqrt(half_length_squared), half_length_squared > 0.00000001);
    let v_dot_h = clamp(dot(view, h), 0.0, 1.0);
    let fresnel = fresnel_schlick(surface.f0, surface.f90, v_dot_h);

    var diffuse_weight = 1.0;
    @if(transmission) {
        diffuse_weight = 1.0 - surface.transmission;
    }
    @if(subsurface) {
        diffuse_weight = diffuse_weight * (1.0 - surface.diffuse_transmission);
        let behind = cosine_integral(corners, -n);
        result.diffuse = surface.diffuse_transmission * surface.diffuse_transmission_color * (1.0 - surface.metallic)
            * (behind / PI) * radiance;
    }
    let irradiance = cosine_integral(corners, n);
    let diffuse = diffuse_weight * front_diffuse(surface, fresnel, max(dot(n, centre), 0.0), n_dot_v, v_dot_h)
        * irradiance;

    var specular = vec3<f32>(0.0);
    if (above_horizon(corners, n)) {
        specular = (surface.f0 * base.scales.x + surface.f90 * base.scales.y) * lobe_integral(corners, n, view, base);
    }

    var base_attenuation = 1.0;
    var coat_specular = vec3<f32>(0.0);
    @if(clearcoat) {
        let coat_n = surface.geometric_normal;
        base_attenuation = 1.0 - surface.clearcoat * (COAT_F0 + (1.0 - COAT_F0) * pow(1.0 - v_dot_h, 5.0));
        if (above_horizon(corners, coat_n)) {
            coat_specular = vec3<f32>(surface.clearcoat * (COAT_F0 * coat.scales.x + coat.scales.y))
                * lobe_integral(corners, coat_n, view, coat);
        }
    }

    result.diffuse += diffuse * base_attenuation * radiance;
    result.specular = (specular * base_attenuation + coat_specular) * radiance;
    return result;
}
