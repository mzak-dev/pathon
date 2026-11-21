# M2-20: kontrakty `rhi-types` — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** bariera tekstury niesie stan poprzedni, `rhi-null` egzekwuje kontrakty z ADR-0020 (stan po `write_*`, `bytes_per_row`), a test konformancji pokazuje, że oba backendy zachowują się tak samo.

**Spec:** [ADR-0020](../../adr/0020-kontrakty-rhi-types.md); M2-20 w [planie](../../plan-implementacji.md). Polityka rozrostu struktur, wątki i współrzędne są w tym tasku tylko dokumentem (bez zmian w kodzie poza testem wątków na null).

## Ograniczenia globalne

- `Command` ≤ 32 bajty (test `command_stays_within_32_bytes`).
- Brak nowych zależności; wszystkie testy (także GPU) zielone; testy grafu przechodzą walidację null; wasm check z CI przechodzi.
- Kod samokomentujący; docs i strony diagnostyk po polsku.

## Review Focus

1. Graf nigdy nie emituje `TextureAccess`, którego `from` nie zgadza się ze stanem śledzonym przez null — także dla zakresu `ALL` nad subresource'ami w różnych stanach, dla importów i dla historii (ADR-0011) między klatkami.
2. Po `write_texture` subresource jest w `COPY_DST`, dane są zachowane; bariera z `from: Access::empty()` po uploadzie → R0103.
3. `bytes_per_row` niewyrównane do 256 w kopii bufor↔tekstura → R0113 na null; to samo w `write_texture` przechodzi; za mały `bytes_per_row` / `rows_per_image` → R0113 w obu.

---

### Task 1: Kontrakty RHI

**Files:** `crates/pathon-rhi-types/src/access.rs` (`Access: u16`), `command.rs` (`TextureAccess { texture, range, from, to }`, `texture_access(texture, range, from, to)`); `crates/pathon-rhi-types/src/error.rs` (+ R0113, jeśli kody tam są); `crates/pathon-rhi-null/src/validator.rs`, `device.rs` (sprawdzanie `from`, `COPY_DST` po `write_texture`, reguły `bytes_per_row` / `rows_per_image`); `crates/pathon-rhi-wgpu/src/encode.rs` (wzorzec dopasowania); `crates/pathon-graph/src/execute.rs` (`StateTracker::transition` podaje `from`; zakres `ALL` tylko gdy wszystkie subresource'y mają ten sam stan poprzedni i docelowy) i inne miejsca w grafie/core emitujące bariery; wszystkie wywołania `texture_access` w testach, przykładach, `pathon-test`, `tools/pathon-devtools`; testy: `crates/pathon-rhi-null/tests/state_validation.rs`, `crates/pathon-rhi/tests/conformance.rs`, `crates/pathon-graph` testy; docs: `docs/crates/pathon-rhi-types.md` (sekcja „Kontrakty”: wszystkie punkty ADR-0020 skrótowo z linkiem), `docs/diag/R0103.md`, nowa `docs/diag/R0113.md` (+ rejestracja kodu, jeśli jest rejestr — test `every_rhi_error_code_is_registered`), `docs/plan-implementacji.md` (odhacz M2-20 ze stanem).

**Interfaces:**
- `Access` jako `bitflags` na `u16`.
- `Command::TextureAccess { texture: TextureId, range: SubresourceRange, from: Access, to: Access }`.
- `CommandList::texture_access(&mut self, texture: TextureId, range: SubresourceRange, from: Access, to: Access)`.
- Kod `R0113` dla niepoprawnego `TexelLayout`.

- [ ] **Step 1: Testy RED:**
  - konformancja (null + wgpu, wzorem istniejących testów w `conformance.rs`, GPU za `gpu-tests`): `write_texture` → bariera `COPY_DST → SAMPLED` → próbkowanie daje zapisane dane; w tym samym passie `set_bind_group` przed `set_pipeline` działa na obu backendach.
  - null: `from` niezgodne ze stanem → R0103; bariera z `from: empty` po `write_texture` → R0103; zakres z mieszanymi stanami i jednym `from` → R0103.
  - null: `copy_buffer_to_texture` / `copy_texture_to_buffer` z `bytes_per_row` = 100 → R0113; `write_texture` z `bytes_per_row` = 100 (wiersz ≤ 100 B) → OK; `bytes_per_row` mniejsze niż wiersz albo `rows_per_image` mniejsze niż wysokość (przy więcej niż jednej warstwie/głębokości) → R0113.
  - null: `create_render_pipeline` z innego wątku (`std::thread::scope`) w trakcie `submit` na tym samym urządzeniu kończy się poprawnie.
  - graf: pass, który zmienia stan jednego mipa tekstury, a potem pass czytający całą teksturę — emitowane bariery przechodzą walidację null (już dziś testy grafu walidują null — dopilnuj przypadku mieszanego).
  - `command_stays_within_32_bytes` zielony.
- [ ] **Step 2: Implementacja** według ADR-0020.
- [ ] **Step 3: Uruchom** `cargo test --workspace --all-features` (GPU), clippy `-D warnings`, wasm check, deps-check.
- [ ] **Step 4: Commit** `feat(rhi)!: carry the previous state in texture barriers and enforce RHI contracts (ADR-0020, M2-20)`
