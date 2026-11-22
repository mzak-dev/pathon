# M3-04: kompilacja w tle — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** nowa permutacja shadera i nowy potok powstają poza wątkiem klatki; klatka tylko zbiera wyniki i pyta o gotowość, a paczka wybiera, czy brakujący draw pomija, czy rysuje zastępczym potokiem.

**Spec:** [ADR-0024](../../adr/0024-kompilacja-w-tle.md), [ADR-0004](../../adr/0004-model-watkow.md), [ADR-0008](../../adr/0008-pula-watkow.md), [ADR-0023](../../adr/0023-permutacje-shaderow.md); M3-04 w [planie](../../plan-implementacji.md).

## Ograniczenia globalne

- Core nie tworzy wątków: crate'y poza `pathon` nie spawnują (xtask `no-threads`); praca idzie przez `TaskPool`. Natywna implementacja w `pathon` (`StdTaskPool`).
- Brak nowych zależności zewnętrznych; baza crate'ów rdzenia bez zmian. `pathon-shader` zależy od `pathon-api`.
- wasm: jeden wątek, `Instant` niedostępny, więc budżet bez wątków to liczba buildów na pump, nie czas.
- Kod samokomentujący, docs po polsku.

## Review Focus

1. Deterministyczny test p99: koszt klatki (pump plus zapytania) przy 40 nowych permutacjach i buildzie 40 ms mieści się w budżecie 10 ms, a test kontrolny (ścieżka blokująca) go łamie; bez flaky timingu (czterokrotny zapas po stronie błędnej, tysiąckrotny po poprawnej, więcej niż 100 klatek).
2. Brak deadlocków: drop kompilatora z buildami zablokowanymi na bramce wraca od razu; zasoby dokończone po dropie są niszczone; zakolejkowane buildy się nie zaczynają.
3. Błąd kompilacji, błąd tworzenia potoku i panika w zadaniu dają `BuildFailure` z kodem S0001 / S0002, klucz zostaje `Failed` bez ponawiania co klatkę, a reszta buildów idzie dalej.
4. Zmiana źródła w trakcie buildu: stary wynik jest niszczony, klucz budowany od nowa na nowym źródle.
5. Pula bez wątków: dokładnie `inline_jobs_per_pump` buildów na pump, wszystko na wątku wołającego.
6. Zmiany poza `pathon-shader` są minimalne i addytywne: `TaskPool::try_spawn` z implementacją domyślną, `SetupCtx` z uchwytami `Arc` (rdzeń przekazuje te, które ma).

---

### Task 1: Kontrakt puli i uchwyty w `SetupCtx`

**Files:** `crates/pathon-api/src/task_pool.rs`, `context.rs`, `lib.rs`; `crates/pathon-core/src/{registry,builder,renderer}.rs`; `crates/pathon/src/runtime/native.rs`; docs `pathon-api.md`, `pathon.md`.

- [x] **Step 1: Testy RED** (`SerialTaskPool` oddaje zadanie; `SetupCtx` oddaje te same uchwyty; `StdTaskPool` uruchamia zadanie na innym wątku).
- [x] **Step 2: Implementacja** `try_spawn` z domyślnym `Err(job)`, `BackgroundJob`, `device_handle()` / `task_pool_handle()`.
- [x] **Step 3: Commit** `feat(api): add TaskPool::try_spawn and owned handles in SetupCtx (ADR-0024, M3-04)`

### Task 2: Kompilator w tle

**Files:** `crates/pathon-shader/src/{background,library,pipeline_cache,lib}.rs`, `Cargo.toml`; `crates/pathon-diag/src/codes.rs`, `docs/diag/S000{1,2}.md`; testy `crates/pathon-shader/tests/{background,background_gpu}.rs`, `tests/common/mod.rs`.

**Interfaces:** `BackgroundCompiler::{new, add_module, add_root, shader_module, render_pipeline, compute_pipeline, wgsl, pump, take_failures, retry_failed, pending, finish_all, release_all}`, `Lookup<T>`, `MissPolicy<T>`, `BackgroundConfig`, `BuildError`, `BuildFailure`, `PumpReport`; `ShaderLibrary::wgsl`.

- [x] **Step 1: Testy RED** z Review Focus (`rhi-null` z opóźnieniem, bramką, wstrzykiwaniem błędów i paniki) oraz test GPU na wgpu.
- [x] **Step 2: Implementacja** według ADR-0024.
- [x] **Step 3: Uruchom** `cargo test --workspace --all-features`, clippy `-D warnings`, fmt, wasm check, deps-check, testy bez featurów pod `-D warnings`.
- [x] **Step 4: Commit** `feat(shader): compile permutations and pipelines in the background (ADR-0024, M3-04)`
