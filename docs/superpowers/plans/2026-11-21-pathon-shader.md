# M3-01: `pathon-shader` z importami — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** shader WGSL z `#import` składa się w jedno źródło WGSL, które RHI przyjmuje i z którego powstaje działający potok.

**Spec:** [ADR-0021](../../adr/0021-importy-shaderow-naga-oil.md), [ADR-0006](../../adr/0006-wejscie-shaderow-do-rhi.md); M3-01 w [planie](../../plan-implementacji.md).

## Ograniczenia globalne

- Nowe zależności tylko w `pathon-shader`: `naga_oil` 0.23 i `naga` 30 (te same wersje co wgpu 30, cecha `wgsl-out` do zapisu WGSL); licencje z allowlisty ADR-0001; `pathon-core` nie zależy od `pathon-shader` w tym tasku (bazowa liczba crate'ów rdzenia bez zmian).
- Crate kompiluje się na `wasm32-unknown-unknown` (wasm check z CI).
- Kod samokomentujący, `#![doc = include_str!("../../../docs/crates/pathon-shader.md")]`, docs po polsku.

## Review Focus

1. Błędny import (nieznany moduł, cykl, błąd składni w module) → `ShaderError` z czytelnym komunikatem, bez paniki.
2. Złożone źródło przechodzi `create_shader_module` na null i na wgpu, a potok z niego daje oczekiwany wynik na GPU.
3. Moduł dodany drugi raz pod tą samą ścieżką importu zastępuje poprzedni (albo jest jawnym błędem) — zachowanie opisane i przetestowane.

---

### Task 1: `pathon-shader`

**Files:** `crates/pathon-shader/Cargo.toml`, `src/lib.rs` (+ moduły, jeśli potrzeba); root `Cargo.toml` (member + wersje w `[workspace.dependencies]`); `xtask/src/rules.rs` (`PINNED_DEPENDENCIES`: `naga_oil` → `pathon-shader`); testy: `crates/pathon-shader/tests/` (składanie, błędy) i test GPU za cechą `gpu-tests` (zależności dev: `pathon-rhi`); docs: `docs/crates/pathon-shader.md`, `docs/plan-implementacji.md` (odhacz M3-01 ze stanem; zaktualizuj wiersz „+ naga_oil” w budżecie zależności zmierzoną liczbą), `README.md` / lista crate'ów, jeśli taka jest.

**Interfaces:**
- `pub struct ShaderComposer` z `new()`, `add_module(&mut self, source: &str, file_path: &str) -> Result<(), ShaderError>` (moduł z `#define_import_path`), `compose(&mut self, source: &str, file_path: &str) -> Result<String, ShaderError>` (zwraca WGSL).
- `#[non_exhaustive] pub enum ShaderError` (`thiserror`), warianty co najmniej dla błędu składania i błędu zapisu WGSL; `Debug`, `Display` z komunikatem naga_oil.

- [ ] **Step 1: Testy RED:**
  - składanie: shader importuje funkcję z modułu `pathon::colors` i stałą z drugiego modułu, który sam importuje pierwszy; wynik parsuje się przez `naga::front::wgsl` bez błędu.
  - błędy: import nieznanego modułu, błąd składni w module, cykl importów → `Err(ShaderError::…)` z nazwą pliku w komunikacie.
  - RHI: złożone źródło → `create_shader_module` i `create_compute_pipeline` na `rhi-null`; GPU (`gpu-tests`): compute z importowaną funkcją zapisuje wartość do bufora storage, odczyt ją potwierdza.
- [ ] **Step 2: Implementacja** według ADR-0021.
- [ ] **Step 3: Uruchom** `cargo test --workspace --all-features`, clippy `-D warnings`, wasm check, deps-check.
- [ ] **Step 4: Commit** `feat(shader): add pathon-shader with WGSL imports via naga_oil (ADR-0021, M3-01)`
