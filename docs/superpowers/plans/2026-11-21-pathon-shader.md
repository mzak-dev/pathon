# M3-01: `pathon-shader` z importami — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** shader WGSL z `#import` składa się w jedno źródło WGSL, które RHI przyjmuje i z którego powstaje działający potok.

**Spec:** [ADR-0021](../../adr/0021-importy-shaderow-wesl.md), [ADR-0006](../../adr/0006-wejscie-shaderow.md); M3-01 w [planie](../../plan-implementacji.md).

## Ograniczenia globalne

- Nowe zależności tylko w `pathon-shader`: `wesl` 0.6 z cechą `naga-ext` (bez naga_oil; `naga` tylko jako zależność dev do walidacji w testach); licencje z allowlisty ADR-0001; `pathon-core` nie zależy od `pathon-shader` w tym tasku (bazowa liczba crate'ów rdzenia bez zmian).
- Crate kompiluje się na `wasm32-unknown-unknown` i jest w jobie wasm w CI.
- Kod samokomentujący, `#![doc = include_str!("../../../docs/crates/pathon-shader.md")]`, docs po polsku.

## Review Focus

1. Błędny import (nieznany moduł, cykl, błąd składni w module) → `ShaderError` z czytelnym komunikatem i ścieżką składanego shadera, bez paniki.
2. Złożone źródło przechodzi `create_shader_module` na null i na wgpu, a potok z niego daje oczekiwany wynik na GPU.
3. Shader główny zachowuje nazwy entry pointów i overrides (także `pass1`, `scale0`), nieużywane overrides oraz `var<immediate>` i `enable`.
4. Ponowne dodanie modułu pod tą samą ścieżką zastępuje go, a złożenie po zmianie widzi nową treść.

---

### Task 1: `pathon-shader` na wesl

Gałąź ma już wersję na naga_oil (commity `1f5adc0`, `447864a`); recenzja odrzuciła ją (`.superpowers/sdd/2026-11-21-pathon-shader/review-1.md`). Zastąp implementację, zachowując to, co nie zależy od naga_oil (struktura crate'u, test GPU, docs), i usuń naga_oil całkowicie.

**Files:** `crates/pathon-shader/Cargo.toml`, `src/lib.rs`; root `Cargo.toml` (`[workspace.dependencies]`); `xtask/src/rules.rs` (`PINNED_DEPENDENCIES`: `wesl` → `pathon-shader`, bez naga_oil); `.github/workflows/ci.yml` (job wasm sprawdza też `-p pathon-shader`); testy: `crates/pathon-shader/tests/` i test GPU za cechą `gpu-tests`; docs: `docs/crates/pathon-shader.md`, `docs/plan-implementacji.md` (stan M3-01, wiersz budżetu zależności „+ naga_oil” → „+ wesl” ze zmierzoną liczbą, opis M3-01), `Cargo.lock`.

**Interfaces:**
- `pub struct ShaderComposer` z `new()`, `add_module(&mut self, path: &str, source: &str) -> Result<(), ShaderError>` (moduł pod ścieżką `path`, importowany jako `package::<path>`; ponowne dodanie zastępuje), `compose(&self, source: &str, file_path: &str) -> Result<String, ShaderError>` (zwraca WGSL; `file_path` tylko do komunikatów).
- `#[non_exhaustive] pub enum ShaderError` (`thiserror`), co najmniej: niepoprawna ścieżka modułu, błąd składania (komunikat wesl + `file_path`).

- [ ] **Step 1: Testy RED:**
  - składanie: shader importuje funkcję z `package::colors` i stałą z drugiego modułu, który sam importuje pierwszy; wynik waliduje `naga` (`Capabilities::all()`).
  - nazwy: entry pointy `pass1`, `layer0` i override `scale0` w shaderze głównym są w wyniku bez zmian; nieużywany override w shaderze głównym zostaje; `override a = 2.0; override b = a * 2.0;` składa się i waliduje.
  - rozszerzenia: `var<immediate>` w shaderze głównym i w module; `enable wgpu_binding_array;` z `binding_array<texture_2d<f32>, 4>`; `enable f16;` w module.
  - render: `@vertex` / `@fragment` z bindingami w grupach 0 i 1 z dwóch modułów; `@id(3) override` w module zachowuje `@id(3)`.
  - błędy: nieznany import, cykl, błąd składni w module, niepoprawna ścieżka modułu → `Err(ShaderError::…)` z `file_path` w komunikacie, bez paniki.
  - zastąpienie: moduł dodany drugi raz → złożenie używa nowej treści.
  - RHI: złożone źródło → `create_shader_module` i `create_compute_pipeline` na `rhi-null`; GPU: compute z importowaną funkcją i overridem z shadera głównego zapisuje wartość do bufora storage, odczyt ją potwierdza.
- [ ] **Step 2: Implementacja** według ADR-0021.
- [ ] **Step 3: Uruchom** `cargo test --workspace --all-features`, clippy `-D warnings`, wasm check (także `pathon-shader`), deps-check.
- [ ] **Step 4: Commit** `feat(shader)!: compose WGSL imports with wesl instead of naga_oil (ADR-0021, M3-01)`
