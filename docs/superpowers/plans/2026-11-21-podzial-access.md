# M2-21: podział `Access::SHADER_READ` — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** dostęp shaderów rozróżnia próbkowanie od storage, graf nadaje teksturom właściwe użycia, a backend null odrzuca stany i bindingi, których natywny backend nie wyrazi.

**Spec:** [ADR-0016](../../adr/0016-podzial-dostepu-shaderow.md); M2-21 w [planie](../../plan-implementacji.md); pozycja #2 w [spike-ash](../../spike-ash.md).

## Ograniczenia globalne

- `SHADER_READ` / `SHADER_WRITE` znikają całkowicie (bez aliasów).
- Brak nowych zależności; wszystkie testy (także GPU) zielone; testy grafu przechodzą walidację null.
- Kod samokomentujący, docs po polsku.

## Review Focus

1. Pass czytający teksturę storage (`STORAGE_READ`), której nikt w klatce nie zapisuje, dostaje `TextureUsage::STORAGE`.
2. Historia (ADR-0011) i cele offscreen kończą klatkę w stanach poprawnych dla tekstury (`is_valid_texture_state`), a odczyt celu przez hosta (testy GPU, `multi_view`) dalej działa.
3. Binding tekstury głębi do próbkowania przy stanie `DEPTH_READ | SAMPLED` przechodzi walidację null.

---

### Task 1: Podział dostępu

**Files:** `crates/pathon-rhi-types/src/access.rs`; `crates/pathon-rhi-null/src/validator.rs`, `device.rs`, `tests/state_validation.rs`; `crates/pathon-rhi/tests/conformance.rs`; `crates/pathon-graph/src/compile.rs` (`usage_for` i użycia buforów), `history.rs`, testy i benchmarki; `crates/pathon-core/src/target.rs` (`OFFSCREEN_FINAL`), `runner.rs`, testy; `crates/pathon-test/src/*.rs`; `tools/pathon-devtools/src/demo.rs`; `examples/` (jeśli używają); docs: `docs/crates/pathon-rhi-types.md`, `docs/crates/pathon-graph.md`, `docs/crates/pathon-core.md` (stan końcowy offscreen), `docs/plan-implementacji.md` (odhacz M2-21).

**Interfaces:**
- `Access::{SAMPLED, STORAGE_READ, STORAGE_WRITE}` zamiast `SHADER_READ`, `SHADER_WRITE`; `WRITES` zawiera `STORAGE_WRITE`.
- `Access::is_valid_state(self) -> bool` (ogólna: jeden rodzaj zapisu; `STORAGE_WRITE` + `STORAGE_READ`, `DEPTH_WRITE` + `DEPTH_READ` dozwolone).
- `Access::is_valid_texture_state(self) -> bool` (dodatkowo: wszystkie bity odczytu w jednej klasie układu — {`SAMPLED`, `DEPTH_READ`}, {`STORAGE_READ`, `STORAGE_WRITE`}, {`COPY_SRC`}, {`COLOR_ATTACHMENT`}, {`DEPTH_READ`, `DEPTH_WRITE`}, {`PRESENT`}, {`COPY_DST`}).
- `OFFSCREEN_FINAL = Access::SAMPLED`.

- [ ] **Step 1: Testy RED:**
  - `tests/graph.rs` `a_pass_reading_a_storage_texture_nobody_writes_gets_storage_usage` (import tekstury albo tekstura tymczasowa z zapisem przez kopię — wybierz wariant, w którym dziś brakuje `STORAGE`; asercja na użycie w opisie fizycznej tekstury / `compiled` usages).
  - `rhi-types` testy jednostkowe: `SAMPLED | DEPTH_READ` poprawne dla tekstury; `SAMPLED | COPY_SRC` i `SAMPLED | STORAGE_READ` odrzucone przez `is_valid_texture_state`; `STORAGE_READ | STORAGE_WRITE` poprawne; `STORAGE_READ | UNIFORM | VERTEX` poprawne przez `is_valid_state` (bufor).
  - `rhi-null` `state_validation.rs`: `TextureAccess` ze stanem `SAMPLED | COPY_SRC` → błąd R0103; binding próbkowanej tekstury przy stanie `STORAGE_READ` → R0101.
- [ ] **Step 2: Implementacja** według ADR-0016 i Interfaces; zamiana wszystkich użyć `SHADER_READ` / `SHADER_WRITE` w workspace na właściwy nowy bit (próbkowanie → `SAMPLED`, storage → `STORAGE_*`).
- [ ] **Step 3: Uruchom** `cargo test --workspace --all-features` (z GPU), clippy `-D warnings`, wasm check z CI, deps-check.
- [ ] **Step 4: Commit** `feat(rhi)!: split shader access into sampled and storage (ADR-0016, M2-21)`
