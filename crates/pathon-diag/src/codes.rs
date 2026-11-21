#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CodeInfo {
    pub code: &'static str,
    pub title: &'static str,
}

const fn code(code: &'static str, title: &'static str) -> CodeInfo {
    CodeInfo { code, title }
}

// R: RHI, G: frame graph, F: render features (packs), P: platform. Every entry needs docs/diag/<code>.md.
pub const CODES: &[CodeInfo] = &[
    code("R0001", "no adapter or device could be created"),
    code("R0002", "adapter lacks required features"),
    code("R0003", "invalid or destroyed handle"),
    code("R0004", "surface lost"),
    code("R0005", "surface image unavailable this frame"),
    code("R0006", "device lost"),
    code("R0007", "out of GPU memory"),
    code("R0008", "unsupported operation"),
    code("R0009", "backend validation error"),
    code("R0101", "resource used in a state that does not allow it"),
    code("R0102", "subresource read before anything wrote it"),
    code("R0103", "invalid access state"),
    code("R0104", "malformed command list"),
    code("R0105", "resource used without the matching usage flag"),
    code("R0106", "surface frame presented without reaching PRESENT"),
    code("R0107", "bind group does not match its layout"),
    code("R0108", "buffer access out of bounds"),
    code("R0109", "invalid texture or view description"),
    code("R0110", "empty shader module"),
    code("R0111", "surface used out of order"),
    code("R0112", "texture range outside its mips or layers"),
    code("G0001", "cycle between passes"),
    code("G0002", "ordering constraint names an unknown pass"),
    code("G0003", "read of a resource with no producer"),
    code("G0004", "render pass mixes render size and output size attachments"),
    code("G0005", "slot written with a format that breaks the slot contract"),
    code("G0006", "pass order decided by registration order"),
    code("G0007", "history declared twice with different descriptions"),
    code("F0101", "feature disabled: missing capabilities"),
    code("F0102", "two features claim the same role"),
    code("F0103", "feature runs its fallback path"),
    code("F0104", "feature failed during setup"),
    code("F0105", "feature failed while recording a frame"),
    code("F0106", "second view of the same target in one frame skipped"),
    code("F0201", "features require different backends"),
    code("P0001", "browser without WebGPU"),
];

pub fn is_registered(code: &str) -> bool {
    CODES.iter().any(|info| info.code == code)
}
