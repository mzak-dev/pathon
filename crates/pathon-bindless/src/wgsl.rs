use std::fmt::Write as _;

use crate::SamplerPreset;

/// Textures one per-material bind group holds. The glTF set is five maps; the rest is room for extension maps
/// (clearcoat, transmission, specular) without a layout change.
const TEXTURE_COUNT: u32 = 8;
pub const MAX_MATERIAL_TEXTURES: usize = TEXTURE_COUNT as usize;

const SLOT_VECTORS: usize = MAX_MATERIAL_TEXTURES / 4;

/// Bindless group: binding 0 is the texture array, samplers follow in the order of [`SamplerPreset::ALL`].
pub(crate) const BINDLESS_TEXTURES_BINDING: u32 = 0;
pub(crate) const BINDLESS_FIRST_SAMPLER_BINDING: u32 = 1;

/// Per-material group: binding 0 is the uniform with the slot of every texture, then the textures, then the samplers.
pub(crate) const MATERIAL_SLOTS_BINDING: u32 = 0;
pub(crate) const MATERIAL_FIRST_TEXTURE_BINDING: u32 = 1;
pub(crate) const MATERIAL_FIRST_SAMPLER_BINDING: u32 = MATERIAL_FIRST_TEXTURE_BINDING + TEXTURE_COUNT;

const SHARED: &str = "
const NO_TEXTURE: u32 = 0xffffffffu;
const SLOT_MASK: u32 = 0x00ffffffu;
const SAMPLER_SHIFT: u32 = 24u;
";

fn sampler_cases(texture: &str, sampler_prefix: &str) -> String {
    let mut cases = String::new();
    for preset in 1..SamplerPreset::COUNT {
        let _ = writeln!(
            cases,
            "        case {preset}u: {{ color = textureSampleGrad({texture}, {sampler_prefix}{preset}, uv, uv_dx, uv_dy); }}"
        );
    }
    let _ = writeln!(
        cases,
        "        default: {{ color = textureSampleGrad({texture}, {sampler_prefix}0, uv, uv_dx, uv_dy); }}"
    );
    cases
}

/// `package::material::texture_source` for the bindless path: the slot indexes one `binding_array`.
pub(crate) fn bindless_texture_source(group: u32, capacity: u32) -> String {
    let mut source = String::from("enable wgpu_binding_array;\n");
    let _ = writeln!(
        source,
        "\n@group({group}) @binding({BINDLESS_TEXTURES_BINDING}) var bindless_textures: binding_array<texture_2d<f32>, {capacity}>;"
    );
    for (preset, binding) in (0..SamplerPreset::COUNT).zip(BINDLESS_FIRST_SAMPLER_BINDING..) {
        let _ = writeln!(
            source,
            "@group({group}) @binding({binding}) var bindless_sampler_{preset}: sampler;"
        );
    }
    source.push_str(SHARED);
    let _ = write!(
        source,
        "
fn sample_material_texture(slot: u32, uv: vec2<f32>, uv_dx: vec2<f32>, uv_dy: vec2<f32>) -> vec4<f32> {{
    if (slot == NO_TEXTURE) {{
        return vec4<f32>(1.0);
    }}
    let index = slot & SLOT_MASK;
    var color: vec4<f32>;
    switch (slot >> SAMPLER_SHIFT) {{
{cases}    }}
    return color;
}}
",
        cases = sampler_cases("bindless_textures[index]", "bindless_sampler_"),
    );
    source
}

/// `package::material::texture_source` for the per-material path: the bind group holds the textures of one material
/// and a uniform that tells which slot is in which binding, so the material data is the same as on the bindless path.
pub(crate) fn per_material_texture_source(group: u32) -> String {
    let mut source = String::new();
    let _ = writeln!(
        source,
        "struct MaterialTextureSlots {{\n    slots: array<vec4<u32>, {SLOT_VECTORS}>,\n}}\n"
    );
    let _ = writeln!(
        source,
        "@group({group}) @binding({MATERIAL_SLOTS_BINDING}) var<uniform> material_texture_slots: MaterialTextureSlots;"
    );
    for (texture, binding) in (0..MAX_MATERIAL_TEXTURES).zip(MATERIAL_FIRST_TEXTURE_BINDING..) {
        let _ = writeln!(
            source,
            "@group({group}) @binding({binding}) var material_texture_{texture}: texture_2d<f32>;"
        );
    }
    for (preset, binding) in (0..SamplerPreset::COUNT).zip(MATERIAL_FIRST_SAMPLER_BINDING..) {
        let _ = writeln!(
            source,
            "@group({group}) @binding({binding}) var material_sampler_{preset}: sampler;"
        );
    }
    source.push_str(SHARED);
    let _ = write!(
        source,
        "
fn sample_with_preset(
    texture: texture_2d<f32>,
    preset: u32,
    uv: vec2<f32>,
    uv_dx: vec2<f32>,
    uv_dy: vec2<f32>,
) -> vec4<f32> {{
    var color: vec4<f32>;
    switch (preset) {{
{cases}    }}
    return color;
}}

fn sample_material_texture(slot: u32, uv: vec2<f32>, uv_dx: vec2<f32>, uv_dy: vec2<f32>) -> vec4<f32> {{
    if (slot == NO_TEXTURE) {{
        return vec4<f32>(1.0);
    }}
    let preset = slot >> SAMPLER_SHIFT;
",
        cases = sampler_cases("texture", "material_sampler_"),
    );
    for texture in 0..MAX_MATERIAL_TEXTURES {
        let (vector, lane) = (texture / 4, ["x", "y", "z", "w"][texture % 4]);
        let _ = write!(
            source,
            "    if (slot == material_texture_slots.slots[{vector}].{lane}) {{
        return sample_with_preset(material_texture_{texture}, preset, uv, uv_dx, uv_dy);
    }}
"
        );
    }
    source.push_str("    return vec4<f32>(1.0);\n}\n");
    source
}
