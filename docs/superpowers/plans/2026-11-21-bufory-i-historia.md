# Bufory i historia w grafie — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** paczki deklarują bufory i historię z poprzedniej klatki w grafie, a graf sam liczy kolejność, culling, aliasing i bariery.

**Architektura:** bufory dochodzą do `ResourceKind` obok tekstur i korzystają z tych samych wersji i łańcuchów dostępów. Historia to para tekstur trwałych w `HistoryStore`, którego runner trzyma per cel; graf czyta z niego ważność poprzedniej klatki i oddaje mu stan `Access` po wykonaniu.

**Stack:** Rust 2024, `pathon-graph`, `pathon-core`, `tools/pathon-devtools`, backend null w testach.

**Spec:** [ADR-0011](../../adr/0011-bufory-i-tekstury-trwale-w-grafie.md)

## Ograniczenia globalne

- API tekstur w `PassBuilder` i `PassContext` się nie zmienia; dochodzą tylko nowe metody.
- `TexturePool` zmienia nazwę na `ResourcePool` (trzyma też bufory) — jedyna zmiana nazwy w publicznym API.
- `pathon-core` nie dostaje nowych zależności (`xtask deps-check`, baseline 126).
- Każdy test grafu przechodzi walidację backendu null (`device.submit(execution.commands)`).
- Kod samokomentujący, komentarze tylko „dlaczego”, opis w `docs/` po polsku (CLAUDE.md).

## Review Focus

1. Pass compute, który tylko zapisuje bufor czytany przez draw indirect, nie może zostać wycięty — Task 1.
2. Bufor zaimportowany kończy klatkę w stanie `final_access`, także gdy żaden pass go nie użył — Task 1.
3. Pierwsza klatka historii: `previous_valid == false`, a odczyt `previous` przechodzi walidację null (graf zeruje teksturę) — Task 2.
4. Zmiana rozmiaru widoku albo `reset` urządzenia unieważnia historię; stare tekstury idą do `DeferredRelease` — Task 2.
5. Widok pominięty w jednej klatce zachowuje historię do następnej — Task 2.

---

### Task 1: Bufory jako zasoby grafu

**Files:**
- Modify: `crates/pathon-graph/src/graph.rs`, `compile.rs`, `execute.rs`, `pool.rs`, `lib.rs`
- Modify (rename): wszystkie użycia `TexturePool` w workspace (`pathon-core`, `tools/pathon-devtools`, `examples`, testy, benchmarki)
- Test: `crates/pathon-graph/tests/graph.rs`

**Interfaces:**
- Produces:
  - `pub struct GraphBufferDesc { pub size: u64, pub usage: BufferUsage }` + `GraphBufferDesc::new(size: u64, usage: BufferUsage) -> Self`.
  - `pub struct BufferHandle` (Copy, Eq, Hash, Debug; jak `TextureHandle`: zasób + wersja).
  - `FrameGraph::create_buffer(&mut self, name: &'static str, desc: GraphBufferDesc) -> BufferHandle`
  - `FrameGraph::import_buffer(&mut self, name: &'static str, buffer: BufferId, size: u64, initial: Access, final_access: Access) -> BufferHandle`
  - `PassBuilder::read_buffer / write_buffer / modify_buffer(&mut self, handle: BufferHandle, access: Access) -> BufferHandle` — semantyka wersji jak `read` / `write` / `modify` tekstur.
  - `PassContext::buffer(&self, handle: BufferHandle) -> BufferId`
  - `ResourcePool` (dawny `TexturePool`) z pulą buforów; bufory tymczasowe o rozłącznych czasach życia i tym samym `(size, usage)` dzielą pamięć, tak jak tekstury.
  - `GraphStats` dostaje `logical_buffers: usize` i `physical_buffers: usize`.
- Wymagane użycie bufora liczone z `Access`: `VERTEX`, `INDEX`, `INDIRECT`, `UNIFORM`, `COPY_SRC`, `COPY_DST` → ta sama flaga `BufferUsage`; `SHADER_READ` / `SHADER_WRITE` → `STORAGE`; suma z `desc.usage`.
- Bariera `Command::BufferAccess` przed passem, gdy stan bufora się zmienia albo poprzedni stan zapisywał (ta sama reguła co `StateTracker::transition` dla tekstur); bufory zaimportowane dostają na końcu przejście do `final_access`. Producent ostatniej wersji zaimportowanego bufora jest korzeniem cullingu.

- [ ] **Step 1: Testy RED** w `tests/graph.rs`:
  - `a_compute_pass_feeding_an_indirect_draw_is_kept_and_ordered_first`: pass A `write_buffer(buf, SHADER_WRITE)`, pass B `read_buffer(buf, INDIRECT)` i zapisuje wyjście; `compiled.order() == ["a", "b"]`; komendy mają `BufferAccess { access: SHADER_WRITE }` przed A i `BufferAccess { access: INDIRECT }` przed B; `device.submit` przechodzi.
  - `a_buffer_nobody_reads_does_not_keep_its_writer`: jedyny pass zapisuje bufor, którego nikt nie czyta → `stats().passes_live == 0` (poza passami z wyjściem).
  - `an_imported_buffer_ends_in_its_final_state`: import z `initial = COPY_DST`, `final = VERTEX`, pass `modify_buffer(.., SHADER_WRITE | SHADER_READ)`; ostatnia komenda bufora to `BufferAccess { access: VERTEX }`.
  - `buffers_with_disjoint_lifetimes_share_memory`: łańcuch trzech buforów jak test tekstur `transient_textures_with_disjoint_lifetimes_share_memory_and_pass_state_validation`; `stats.logical_buffers == 3`, `stats.physical_buffers == 2`.
- [ ] **Step 2: Uruchom** `cargo test -p pathon-graph --test graph buffer` — nie kompiluje się / pada.
- [ ] **Step 3: Implementacja** według Interfaces; zmiana nazwy `TexturePool` → `ResourcePool` w całym workspace.
- [ ] **Step 4: Uruchom** `cargo test --workspace` — zielone.
- [ ] **Step 5: Docs** `docs/crates/pathon-graph.md`: sekcja „Bufory” (dwa–trzy zdania).
- [ ] **Step 6: Commit** `feat(graph)!: track buffers as graph resources (ADR-0011)`

### Task 2: Historia per widok

**Files:**
- Create: `crates/pathon-graph/src/history.rs`
- Modify: `crates/pathon-graph/src/graph.rs`, `compile.rs`, `execute.rs`, `lib.rs`; `crates/pathon-core/src/runner.rs`
- Test: `crates/pathon-graph/tests/graph.rs`, `crates/pathon-core/tests/renderer.rs`

**Interfaces:**
- Consumes: `ResourcePool` z Task 1.
- Produces:
  - `pub struct HistoryTexture { pub previous: TextureHandle, pub current: TextureHandle, pub previous_valid: bool }`
  - `pub struct HistoryStore` (Default, Debug) — para tekstur na nazwę, z opisem, rozmiarem i stanem `Access` każdej; `HistoryStore::new() -> Self`, `HistoryStore::forget_all(&mut self)` (po utracie urządzenia, bez niszczenia), `HistoryStore::release_all(&mut self, release: &mut DeferredRelease, frame: u64)`.
  - `FrameGraph::for_view(history: &HistoryStore, sizes: FrameSizes) -> FrameGraph` — graf, który zna historię widoku; `FrameGraph::new()` zostaje (graf bez historii).
  - `FrameGraph::history_texture(&mut self, name: &'static str, desc: GraphTextureDesc) -> HistoryTexture` — `previous_valid` jest `true`, gdy w `HistoryStore` istnieje para o tej nazwie z tym samym opisem i rozmiarem; w grafie z `FrameGraph::new()` zawsze `false`.
  - `FrameGraph::execute(self, compiled, device, pool: &mut ResourcePool, history: &mut HistoryStore, release, frame)` — dodatkowy parametr `history`.
- Zachowanie w `execute`:
  - brakująca albo nieaktualna para jest tworzona na nowo (stara do `release`); tekstury historii zawsze mają też `RENDER_TARGET`;
  - nieważna `previous` jest zerowana przed pierwszym odczytem (kolor: przezroczysta czerń, głębia: 0.0);
  - pass zapisujący ostatnią wersję `current` jest korzeniem cullingu;
  - po klatce stan `Access` obu tekstur trafia do `HistoryStore`, a role `previous` / `current` się zamieniają.
- `GraphRunner` trzyma `HashMap<TargetId, HistoryStore>`: graf widoku powstaje przez `FrameGraph::for_view(&history_of_target, sizes)`; `forget_target` oddaje tekstury do `release`; `forget_device` woła `forget_all`.

- [ ] **Step 1: Testy RED** w `tests/graph.rs`:
  - `history_is_invalid_on_the_first_frame_and_valid_on_the_next`: klatka 1 `previous_valid == false`, pass czyta `previous` (SHADER_READ) i zapisuje `current` (COLOR_ATTACHMENT); submit przechodzi walidację null; klatka 2 z tym samym `HistoryStore` → `previous_valid == true`, `ResourcePool`/store nie tworzą nowych tekstur.
  - `resizing_invalidates_history`: klatka 2 z innym `FrameSizes` → `previous_valid == false`, `release.len()` rośnie o 2.
  - `the_pass_writing_history_is_not_culled`: jedyny pass zapisuje `current`, nikt w tej klatce go nie czyta → pass jest w `compiled.order()`.
- [ ] **Step 2: Test RED** w `tests/renderer.rs` `history_survives_a_skipped_frame_and_resets_with_the_device`: paczka zapisuje w `record` wartość `previous_valid`; klatki `[a]`, `[]` (widok pominięty), `[a]` → `[false, true]`; po `recover()` (utrata urządzenia na null, jak w istniejącym teście device loss) następna klatka → `false`.
- [ ] **Step 3: Uruchom** — nie kompiluje się / pada.
- [ ] **Step 4: Implementacja** według Interfaces.
- [ ] **Step 5: Uruchom** `cargo test --workspace` — zielone.
- [ ] **Step 6: Docs** `docs/crates/pathon-graph.md`: sekcja „Historia”; odhacz M2-08 w `docs/plan-implementacji.md` z dopiskiem, że N = 1 i pula na maksymalny rozmiar są odłożone (ADR-0011).
- [ ] **Step 7: Commit** `feat(graph): keep per-view history textures across frames (ADR-0011, M2-08)`

### Task 3: Bufory i historia w capture i devtools

**Files:**
- Modify: `crates/pathon-graph/src/capture.rs`, `execute.rs`; `tools/pathon-devtools/src/protocol.rs`, `capture.rs`, `ui/panels.rs`
- Test: `crates/pathon-graph/tests/graph.rs`, testy w `tools/pathon-devtools`

**Interfaces:**
- Consumes: zasoby z Task 1 i 2.
- Produces:
  - `pub enum CapturedKind { Texture, Buffer, History }` w `capture.rs`; `ResourceCapture` dostaje `kind: CapturedKind` i `buffer_size: Option<u64>`; `BarrierCapture` dostaje `buffer: Option<BufferId>` (dla barier buforów `texture` nieustawione — rozwiąż przez `enum CapturedTarget { Texture(TextureId), Buffer(BufferId) }` w polu `target`, zastępując `texture`).
  - `ResourceSnapshot` w devtools: `kind: String` („tekstura”, „bufor”, „historia”) i `size` dla bufora jako bajty; zakładka zasobów pokazuje rodzaj w kolumnie „rodzaj”.

- [ ] **Step 1: Test RED** w `tests/graph.rs` `capture_lists_buffers_and_history`: graf z buforem i historią z `set_capture(true)` → zasoby mają `CapturedKind::Buffer` i `CapturedKind::History`, bariera bufora ma `CapturedTarget::Buffer`.
- [ ] **Step 2: Test RED** w devtools: demo z buforem (dodaj do `DebugMask` albo nowej paczki demo zapis bufora) → zakładka zasobów zawiera „bufor”.
- [ ] **Step 3: Implementacja**, potem `cargo test --workspace`.
- [ ] **Step 4: Commit** `feat(devtools): show buffers and history textures`
