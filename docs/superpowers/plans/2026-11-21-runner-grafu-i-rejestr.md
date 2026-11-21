# Runner grafu, rejestr paczek i tożsamość widoku — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** `Renderer` przestaje prowadzić przebieg grafu i grzebać w rejestrze; widok ma stabilną tożsamość równą celowi.

**Architektura:** `Registry` dostaje prywatne pola i dwie fazy (`prepare_view`, `record_view`). Nowy moduł `GraphRunner` (`crates/pathon-core/src/runner.rs`) przejmuje pulę tekstur, cache grafów per `TargetId`, czyszczenie celu, fallback i capture. `ViewId` staje się uchwytem celu w `u64`.

**Stack:** Rust 2024, `pathon-core`, `pathon-graph`, `pathon-api`, backend null w testach.

**Spec:** [ADR-0010](../../adr/0010-tozsamosc-widoku-i-runner-grafu.md)

## Ograniczenia globalne

- Żadna zmiana w API paczek poza `ViewId(pub u64)` (ADR-0010).
- `pathon-core` nie dostaje nowych zależności (`xtask deps-check`, baseline 126).
- Diagnostyki z klatki trafiają do `FrameReport::diagnostics` i są emitowane dokładnie raz.
- Kod samokomentujący, komentarze tylko „dlaczego”, opis w `docs/` (CLAUDE.md).

## Review Focus

1. Dwa widoki z tym samym celem w jednej klatce: obie klatki renderują się bez paniki (drugi nadpisuje pierwszy) — test w Task 2.
2. Cel usunięty i dodany ponownie: nowy `ViewId`, stary cache grafu znika — test w Task 3, `forget_target` w Task 2.
3. Widok pominięty w klatce (zminimalizowane okno, lusterko co drugą klatkę) nie psuje cache innych widoków — pokrywa test przestawiania widoków w Task 2.
4. `reset` / `recover` po utracie urządzenia: pula tekstur zapomniana, klatka po resecie się renderuje — istniejący test `device_loss_rebuilds_the_device_and_asks_for_data_again`.
5. F0105 z paczki i z passa w jednej klatce: każda diagnostyka raz w raporcie i raz w logu — test w Task 1.

---

### Task 1: Rejestr z prywatnymi polami i fazami

**Files:**
- Modify: `crates/pathon-core/src/registry.rs`
- Modify: `crates/pathon-core/src/renderer.rs` (`active_features`, `record_view`)
- Test: `crates/pathon-core/tests/renderer.rs`

**Interfaces:**
- Produces:
  - `Registry::prepare_view(&mut self, view: &mut ViewCtx, shared: &Shared)` — `prepare_view` aktywnych paczek w kolejności, z `owns_temporal` dla właściciela roli temporalnej.
  - `Registry::record_view(&mut self, ctx: &mut RecordCtx<'_>) -> Vec<Diagnostic>` — `record` aktywnych paczek; te, które zwrócą błąd, wyłącza (status, kolejność, rola temporalna) i zwraca ich F0105 **bez emitowania**.
  - `Registry::active_names(&self) -> Vec<&'static str>`.
  - Pola `entries`, `order`, `temporal_owner` prywatne.

- [ ] **Step 1: Test** `a_failing_pack_is_reported_once_per_frame` w `tests/renderer.rs`: `FailsToRecord` (już jest w pliku), jedna klatka, `report.diagnostics` zawiera dokładnie jedną diagnostykę z kodem `"F0105"`.
- [ ] **Step 2: Uruchom** `cargo test -p pathon-core --test renderer reported_once` — przechodzi już dziś; to siatka bezpieczeństwa refaktoru, nie RED.
- [ ] **Step 3: Implementacja** faz w `registry.rs`; `disable` dalej emituje (faza budowy), nowa prywatna `mark_disabled(&mut self, index: usize, diagnostic: &Diagnostic)` tylko zmienia stan i jest używana przez `record_view`. `Renderer` korzysta wyłącznie z faz i `active_names`.
- [ ] **Step 4: Uruchom** `cargo test -p pathon-core` — wszystkie zielone.
- [ ] **Step 5: Commit** `refactor(core): let the registry run its phases and keep its own invariants`

### Task 2: GraphRunner z cache per cel

**Files:**
- Create: `crates/pathon-core/src/runner.rs`
- Modify: `crates/pathon-core/src/renderer.rs`, `crates/pathon-core/src/lib.rs` (`mod runner;`)
- Test: `crates/pathon-core/tests/renderer.rs`, testy jednostkowe w `runner.rs`

**Interfaces:**
- Consumes: `Registry::record_view` z Task 1.
- Produces:
  - `pub(crate) struct OutputTarget` (przeniesiony z `renderer.rs`, bez zmian pól).
  - `pub(crate) struct FrameEnv<'frame> { device: &'frame dyn DeviceApi, release: &'frame mut DeferredRelease, frame: u64 }`.
  - `pub(crate) struct ViewRun { commands: CommandList, stats: GraphStats, compiled: bool, capture: Option<GraphCapture>, diagnostics: Vec<Diagnostic> }` — diagnostyki nieemitowane; najpierw te z `fill`, potem błędy kompilacji i ostrzeżenia grafu.
  - `GraphRunner::new(clear_color: Color) -> Self`, `set_capture(&mut self, enabled: bool)`, `forget_target(&mut self, target: TargetId)`, `forget_device(&mut self)`.
  - `GraphRunner::run_view(&mut self, target: TargetId, output: OutputTarget, sizes: FrameSizes, fill: impl FnOnce(&mut FrameGraph, TextureHandle) -> Vec<Diagnostic>, env: FrameEnv<'_>) -> Result<ViewRun, RenderError>` — import celu (czyszczenie albo nietknięty obraz hosta), `fill`, kompilacja przez cache celu, wykonanie, fallback przy błędzie grafu lub passa; `RhiError::DeviceLost` → `RenderError::DeviceLost`.

- [ ] **Step 1: Test RED** `reordering_views_does_not_recompile_their_graphs`: dwa cele offscreen o różnych rozmiarach (64×32 i 32×16), klatka z `[a, b]`, potem `[b, a]`; w drugiej `report.compiled_views == 0`.
- [ ] **Step 2: Test** `two_views_on_one_target_still_render`: dwa widoki z tym samym celem, `render_frame` zwraca `Ok` z `report.views.len() == 2`.
- [ ] **Step 3: Uruchom** — pierwszy pada (`compiled_views` = 2, cache po pozycji), drugi przechodzi.
- [ ] **Step 4: Test jednostkowy** w `runner.rs` `a_graph_that_fails_to_compile_still_clears_the_target`: `NullDevice`, cel offscreen, `fill` dodaje pass czytający niezapisaną teksturę; `run_view` zwraca `Ok`, `diagnostics[0].code == "G0003"`, komendy zawierają `PushDebugGroup("clear output")`, a `device.submit(commands)` przechodzi walidację.
- [ ] **Step 5: Implementacja** `runner.rs`; cache w `HashMap<TargetId, GraphCache>`. `Renderer` traci pola `pool`, `caches`, `capture`, `clear_color` i metody `import_output`, `add_output_clear`, `record_fallback`; `remove_target` woła `forget_target`, `reset` woła `forget_device`.
- [ ] **Step 6: Uruchom** `cargo test -p pathon-core` i `cargo test -p pathon-devtools` — zielone.
- [ ] **Step 7: Commit** `refactor(core): move the frame graph run into GraphRunner, cached per target`

### Task 3: ViewId z celu

**Files:**
- Modify: `crates/pathon-api/src/view.rs` (`ViewId(pub u64)`), `crates/pathon-core/src/renderer.rs`
- Test: `crates/pathon-core/tests/renderer.rs`

**Interfaces:**
- Produces: `fn view_id(target: TargetId) -> ViewId` w `renderer.rs` — `(generation << 32) | index` z `RawHandle`.

- [ ] **Step 1: Test RED** `a_view_keeps_its_id_while_its_target_lives`: paczka zapisuje `ViewCtx::view` z `prepare_view` do `Arc<Mutex<Vec<ViewId>>>`; klatki `[a, b]` i `[b, a]` dają dla `a` to samo id; po `remove_target(a)` i nowym celu na jego miejscu id jest inne.
- [ ] **Step 2: Uruchom** — pada (id po pozycji).
- [ ] **Step 3: Implementacja** `view_id` i `ViewId(pub u64)`.
- [ ] **Step 4: Uruchom** `cargo test --workspace` — zielone.
- [ ] **Step 5: Docs** `docs/crates/pathon-core.md` (sekcja „Klatka”: cache i `ViewId` per cel, odwołanie do ADR-0010), `docs/crates/pathon-api.md` (`ViewId` = cel).
- [ ] **Step 6: Commit** `feat(core)!: derive ViewId from the view's target (ADR-0010)`

### Task 4: Weryfikacja i publikacja

- [ ] `cargo +nightly fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`, `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`, `cargo xtask deps-check`, `cargo check --target wasm32-unknown-unknown -p pathon-core -p pathon-test -p pathon --no-default-features` — wszystko zielone.
- [ ] Commit ADR-0010 i tego planu: `docs: add ADR-0010 on view identity and the graph runner`; push na `main`, CI zielone.
