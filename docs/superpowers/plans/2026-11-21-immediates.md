# M2-23: immediates i overrides — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** paczka przekazuje małe dane per draw przez immediates i wybiera wariant shadera przez override; null odrzuca to, czego wgpu nie przyjmie.

**Spec:** [ADR-0018](../../adr/0018-immediates-i-overrides.md); M2-23 w [planie](../../plan-implementacji.md).

## Ograniczenia globalne

- `Command` ≤ 32 bajty (test `command_stays_within_32_bytes`).
- Brak nowych zależności; wszystkie testy (także GPU) zielone; wasm check z CI przechodzi.
- Kod samokomentujący; docs i strony diagnostyk po polsku.

## Review Focus

1. `set_immediates` poza zakresem `immediate_size`, bez potoku albo z niewyrównanym offsetem → błąd null, bez paniki.
2. Potok z `immediate_size > 0` na urządzeniu bez `IMMEDIATES` → czytelny błąd na null i na wgpu.
3. Override o nazwie, której shader nie ma → błąd tworzenia potoku (wgpu), nie panika.

---

### Task 1: Immediates i overrides

**Files:** `crates/pathon-rhi-types/src/caps.rs`, `pipeline.rs`, `command.rs`; `crates/pathon-rhi-wgpu/src/convert.rs`, `device.rs`, `encode.rs`; `crates/pathon-rhi-null/src/device.rs`, `validator.rs`; literały opisów potoków w workspace (`pathon-test`, testy, przykłady); testy: `crates/pathon-rhi-null/tests/state_validation.rs`, test GPU w `crates/pathon-rhi/tests/` (feature `gpu-tests`); docs: `docs/crates/pathon-rhi-types.md`, strona diagnostyki dla nowego błędu null (istniejący kod, jeśli pasuje, np. R0104/R0107 — wybierz i opisz), `docs/plan-implementacji.md` (odhacz M2-23).

**Interfaces:**
- `Features::IMMEDIATES`, `Limits::max_immediate_size: u32`.
- `RenderPipelineDesc::immediate_size: u32`, `ComputePipelineDesc::immediate_size: u32`.
- `ShaderStage::constants: Vec<(&'static str, f64)>`.
- `RenderPassEncoder::set_immediates(&mut self, offset: u32, data: &[u8])`, to samo w `ComputePassEncoder`; `Command::SetImmediates { offset: u32, data: Box<[u8]> }`.

- [ ] **Step 1: Testy RED:**
  - GPU: compute shader z `var<immediate> value: u32;` i `override scale: f32;` zapisuje `value` i `scale * 2.0` do bufora storage; urządzenie z `Features::IMMEDIATES`; odczyt pokazuje przekazaną wartość immediate (np. 1234) i override (np. `scale = 3.0` → 6.0).
  - null: `set_immediates` z `offset + len > immediate_size` → błąd; bez ustawionego potoku → błąd; niewyrównany offset → błąd; potok z `immediate_size > 0` bez `IMMEDIATES` → błąd.
  - `command_stays_within_32_bytes` dalej zielony.
- [ ] **Step 2: Implementacja** według ADR-0018.
- [ ] **Step 3: Uruchom** `cargo test --workspace --all-features`, clippy `-D warnings`, wasm check, deps-check.
- [ ] **Step 4: Commit** `feat(rhi)!: add immediates and pipeline overrides (ADR-0018, M2-23)`
