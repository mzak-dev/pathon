# M2-22: tablice bindingów — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** paczka może zadeklarować tablicę tekstur i samplerów w bind groupie i czytać jej elementy w shaderze; null odrzuca grupy, których wgpu nie przyjmie.

**Spec:** [ADR-0017](../../adr/0017-tablice-bindingow.md); M2-22 w [planie](../../plan-implementacji.md).

## Ograniczenia globalne

- Brak nowych zależności; wszystkie testy (także GPU) zielone; wasm check z CI przechodzi.
- Kod samokomentujący, docs i strony diagnostyk po polsku.

## Review Focus

1. Grupa z tablicą i uniform bufferem jest odrzucana przez null (R0107) — i nie dochodzi do wgpu.
2. Tablica krótsza niż `count` bez `PARTIALLY_BOUND_BINDING_ARRAY` → R0107; z tą funkcją → przechodzi.
3. Urządzenie bez `TEXTURE_BINDING_ARRAY` (np. profil Web / null bez funkcji) odrzuca layout z `count` czytelnym błędem, bez paniki.

---

### Task 1: Tablice bindingów

**Files:** `crates/pathon-rhi-types/src/binding.rs`, `caps.rs` (`Limits::max_binding_array_sampler_elements_per_shader_stage` + profile); `crates/pathon-rhi-wgpu/src/device.rs`, `convert.rs` (layout `count`, `BindingResource::TextureViewArray` / `SamplerArray`, limit z adaptera); `crates/pathon-rhi-null/src/device.rs`, `validator.rs` (reguły z ADR-0017, śledzenie użycia elementów tablicy jak pojedynczych bindingów); wszystkie literały `BindGroupLayoutEntry` / użycia `Copy` w workspace; testy: `crates/pathon-rhi-null/tests/state_validation.rs`, `crates/pathon-core/tests/vertical_slice_gpu.rs` (albo test GPU w `crates/pathon-rhi/tests/conformance.rs`); docs: `docs/crates/pathon-rhi-types.md`, `docs/diag/R0107.md`, `docs/plan-implementacji.md` (odhacz M2-22).

**Interfaces:**
- `BindGroupLayoutEntry { binding, visibility, ty, count: Option<NonZeroU32> }`.
- `BindingResource::{TextureViewArray(Vec<TextureViewId>), SamplerArray(Vec<SamplerId>)}`.
- `Limits::max_binding_array_sampler_elements_per_shader_stage: u32`.

- [ ] **Step 1: Testy RED:**
  - GPU: urządzenie z `Features::TEXTURE_BINDING_ARRAY`; 1024 tekstury 1×1 z różnymi wartościami (np. `write_texture` z indeksem w kanale R), compute shader WGSL z `binding_array<texture_2d<f32>, 1024>` czyta element 700 i 3 przez `textureLoad` i zapisuje do bufora storage; odczyt bufora pokazuje wartości 700 i 3 (Gotowe gdy z planu).
  - null: grupa z tablicą i uniform bufferem → R0107; tablica krótsza niż `count` bez `PARTIALLY_BOUND_BINDING_ARRAY` → R0107, z funkcją → OK; layout z `count` bez `TEXTURE_BINDING_ARRAY` → błąd; zasób tablicowy przy wpisie bez `count` → R0107.
- [ ] **Step 2: Implementacja** według ADR-0017.
- [ ] **Step 3: Uruchom** `cargo test --workspace --all-features`, clippy `-D warnings`, wasm check z CI, deps-check.
- [ ] **Step 4: Commit** `feat(rhi)!: add binding arrays to bind group layouts and resources (ADR-0017, M2-22)`
