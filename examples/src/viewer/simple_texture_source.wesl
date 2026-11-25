// The profile "Web" of the material textures (M4-04 replaces this with a `binding_array` on Desktop): every material
// owns a bind group with its five maps in the order of `TextureRole`, so the slot a material reads is the role itself.
// Each map has a UV transform (KHR_texture_transform): two vectors, the 2x2 matrix in row-major order and the offset.

@group(1) @binding(0) var material_base_color: texture_2d<f32>;
@group(1) @binding(1) var material_metallic_roughness: texture_2d<f32>;
@group(1) @binding(2) var material_normal: texture_2d<f32>;
@group(1) @binding(3) var material_occlusion: texture_2d<f32>;
@group(1) @binding(4) var material_emissive: texture_2d<f32>;
@group(1) @binding(5) var material_sampler: sampler;

struct UvTransforms {
    vectors: array<vec4<f32>, 10>,
}

@group(1) @binding(6) var<uniform> uv_transforms: UvTransforms;

fn sample_material_texture(slot: u32, uv: vec2<f32>, uv_dx: vec2<f32>, uv_dy: vec2<f32>) -> vec4<f32> {
    let matrix = uv_transforms.vectors[slot * 2u];
    let offset = uv_transforms.vectors[slot * 2u + 1u].xy;
    let linear = mat2x2<f32>(vec2<f32>(matrix.x, matrix.z), vec2<f32>(matrix.y, matrix.w));
    let coords = linear * uv + offset;
    let dx = linear * uv_dx;
    let dy = linear * uv_dy;
    switch (slot) {
        case 0u: {
            return textureSampleGrad(material_base_color, material_sampler, coords, dx, dy);
        }
        case 1u: {
            return textureSampleGrad(material_metallic_roughness, material_sampler, coords, dx, dy);
        }
        case 2u: {
            return textureSampleGrad(material_normal, material_sampler, coords, dx, dy);
        }
        case 3u: {
            return textureSampleGrad(material_occlusion, material_sampler, coords, dx, dy);
        }
        case 4u: {
            return textureSampleGrad(material_emissive, material_sampler, coords, dx, dy);
        }
        default: {
            return vec4<f32>(1.0);
        }
    }
}
