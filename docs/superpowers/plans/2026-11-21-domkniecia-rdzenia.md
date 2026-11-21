# Domknięcia rdzenia — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** zamknąć otwarte decyzje z przeglądów i drobne zadania M2: duplikat celu, powrót paczek po resecie, eksport grafu do DOT, przykład multi-view.

**Architektura:** zmiany w `pathon-core` (renderer, rejestr), `pathon-graph` (eksport DOT z `GraphCapture`), devtools (zapis DOT) i `examples`.

**Stack:** Rust 2024, backend null w testach, winit w przykładzie.

**Spec:** [ADR-0013](../../adr/0013-dwa-widoki-na-celu-i-paczki-po-resecie.md); M2-13 i M2-17 w [planie implementacji](../../plan-implementacji.md).

## Ograniczenia globalne

- Brak zmian w API paczek (ADR-0012).
- Brak nowych zależności w `pathon-core` i `pathon-graph`.
- Diagnostyki klatki trafiają do `FrameReport::diagnostics` i są emitowane raz.
- Kod samokomentujący, docs i strony diagnostyk po polsku.

## Review Focus

1. Duplikat celu w `record_frame` (tryb A) zachowuje się jak w `render_frame` — Task 1.
2. Przywrócona paczka po resecie nie odbiera roli paczce, która ją przejęła; zasady ról jak przy budowie — Task 2.
3. DOT jest poprawny dla nazw z cudzysłowami i spacjami — Task 3.

---

### Task 1: Drugi widok z tym samym celem

**Files:** `crates/pathon-core/src/renderer.rs`, `crates/pathon-diag/src/codes.rs`, `docs/diag/F0106.md`, `docs/adr/0010-tozsamosc-widoku-i-runner-grafu.md` (Konsekwencje: odesłanie do ADR-0013), `docs/crates/pathon-core.md`; test `crates/pathon-core/tests/renderer.rs`.

- [ ] **Step 1: Testy RED:** `a_second_view_of_the_same_window_is_skipped_with_f0106` (okno `NoWindow` dwa razy w `FrameInput`: `render_frame` → `Ok`, `skipped_views == 1`, `views.len() == 1`, diagnostyka F0106; kolejna klatka też `Ok`); `a_second_view_of_the_same_imported_texture_is_skipped_in_mode_a` (`record_frame`, cel z `import_texture` dwa razy → `Ok`, jedno „clear output”/jeden przebieg, F0106 nie przepada — sprawdź przez `renderer.features()`-niezależny sposób: np. zwracaj F0106 także w trybie A przez log; opisz w raporcie, jak test obserwuje diagnostykę). Istniejący test `two_views_on_one_target_still_render` zmienia oczekiwanie na `views.len() == 1` i `skipped_views == 1`.
- [ ] **Step 2: Implementacja** w `record_views` i `record_frame`: zbiór celów użytych w tej klatce; duplikat → `report.skipped_views += 1` i `Diagnostic::warning("F0106", ...)` przez `Renderer::report`.
- [ ] **Step 3: Docs** strona `docs/diag/F0106.md`, wpis w `codes.rs`, ADR-0010 i `docs/crates/pathon-core.md`.
- [ ] **Step 4: Commit** `fix(core): skip a second view of the same target with F0106 (ADR-0013)`

### Task 2: Paczki wyłączone z F0105 wracają po resecie

**Files:** `crates/pathon-core/src/registry.rs`, `renderer.rs`; test `crates/pathon-core/tests/renderer.rs`; `docs/crates/pathon-core.md`.

- [ ] **Step 1: Testy RED:** `a_pack_disabled_while_recording_comes_back_after_reset` (paczka zawodzi w `record` tylko na pierwszym urządzeniu — np. flaga w `Arc<AtomicBool>` — klatka → F0105; `simulate_device_loss` + `recover()`; `active_features()` znowu zawiera paczkę, jej `setup` wywołany drugi raz, kolejna klatka bez F0105); `a_pack_disabled_for_other_reasons_stays_off_after_reset` (drugi GeometryPack z F0102 po `recover()` dalej F0102).
- [ ] **Step 2: Implementacja:** w `Registry::reset` paczki ze statusem `Disabled { code: "F0105", .. }` wracają do `Active`, przechodzą `setup` (nie `on_device_reset`) i dołączają do kolejności przez `sort` i reguły ról; paczki, które działały, dostają tylko `on_device_reset`. Błąd `setup` przywróconej paczki → F0104.
- [ ] **Step 3: Docs** `docs/crates/pathon-core.md` (sekcja „Utrata urządzenia”).
- [ ] **Step 4: Commit** `fix(core): bring packs disabled while recording back after a device reset (ADR-0013)`

### Task 3: Eksport grafu do DOT (M2-13)

**Files:** `crates/pathon-graph/src/capture.rs` (albo nowy `dot.rs`), `lib.rs`; `tools/pathon-devtools/src/ui.rs` (klawisz `d` zapisuje `pathon-graph-<klatka>.dot` obok `s`), `docs/devtools.md`, `docs/crates/pathon-graph.md` (także usuń nieaktualne zdanie o M2-08 „poza ścieżką minimalną” w sekcji „Nagrywanie”), `docs/plan-implementacji.md` (odhacz M2-13 z dopiskiem, że JSON jest w devtools); test `crates/pathon-graph/tests/graph.rs`.

**Interfaces:** `pub fn to_dot(capture: &GraphCapture) -> String` re-eksportowane z `pathon-graph`; passy wycięte szare i przerywane, krawędzie z `capture.edges`, etykieta passa: nazwa + etap + `#kolejność`.

- [ ] **Step 1: Test RED** `dot_lists_passes_edges_and_culled_passes`: capture z testu `capture_describes_order_culling_dependencies_aliasing_and_barriers` → DOT zaczyna się od `digraph`, zawiera każdą nazwę passa, krawędź `p0 -> p1`, styl `dashed` dla wyciętego; nazwa z cudzysłowem jest escapowana.
- [ ] **Step 2: Implementacja**, potem devtools `d` i test UI w devtools (zapis do katalogu tymczasowego albo funkcja zwracająca ścieżkę — bez śmiecenia w repo).
- [ ] **Step 3: Commit** `feat(graph): export captured graphs to DOT (M2-13)`

### Task 4: Przykład `multi_view` (M2-17)

**Files:** `examples/src/bin/multi_view.rs`, `docs/examples.md`, `docs/plan-implementacji.md` (M2-17: odhacz z dopiskiem, że działa na Windows i buduje się w CI na Linux/macOS).

- [ ] **Step 1: Implementacja** na wzór `examples/src/bin/external_window.rs` (host ma pętlę i okna, bez runnera): dwa okna z `UnlitMeshPack` i różnymi kamerami + cel offscreen 256×256 z trzecią kamerą; co 120 klatek odczyt jednego piksela offscreen (`target_texture` + kopia do bufora, jak w `crates/pathon-core/tests/vertical_slice_gpu.rs`) i `log::info!` z wartością.
- [ ] **Step 2: Uruchom** `cargo build -p pathon-examples --bin multi_view` i krótko uruchom przykład (ok. 3 s, potem zamknij proces), sprawdź log z pikselem.
- [ ] **Step 3: Commit** `feat(examples): add multi_view with two windows and an offscreen target (M2-17)`
