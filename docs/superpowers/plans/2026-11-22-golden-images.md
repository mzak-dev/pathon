# M4-00: golden images — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** jedna funkcja `assert_golden(name, &image, tolerance)` porównuje obraz z renderu headless z referencją PNG w repozytorium, działa z dowolnym urządzeniem i hostem, a pierwszy golden test (wycinek z M2-18) idzie w CI na lavapipe, WARP i macOS.

**Spec:** [ADR-0025](../../adr/0025-golden-images.md); M4-00 w [planie](../../plan-implementacji.md); strona crate'u: [pathon-golden](../../crates/pathon-golden.md).

## Ograniczenia globalne

- Crate `pathon-golden` tylko deweloperski: bez wpływu na domknięcie `pathon-core` (`cargo xtask deps-check`), jedyna nowa zależność to `png`.
- Brak adaptera GPU nigdy nie daje po cichu zielonego testu w zadaniach, które mają GPU (`PATHON_REQUIRE_GPU=1`).
- Kod samokomentujący, docs po polsku.

## Review Focus

1. Test negatywny: celowo zmieniony obraz jest odrzucany, raport zawiera nazwę, liczbę pikseli, pierwszy różny piksel i ścieżki; artefakty `actual` / `expected` / `diff` istnieją.
2. Granice progu: delta kanału i odsetek pikseli działają niezależnie (1 z 100 pikseli mieści się w 1%, nie mieści w 0,5%); brak referencji i inny rozmiar to osobne, czytelne błędy.
3. Odczyt tekstury znosi wiersze dopełnione do 256 B (szerokość 10 px) i zamienia BGRA na RGBA — na prawdziwym GPU.
4. Nazwa golden nie wychodzi poza katalog referencji.
5. CI: zadania lavapipe i WARP mają `PATHON_REQUIRE_GPU=1`; macOS pomija test z widocznym ostrzeżeniem, jeśli nie ma Metala.

---

### Task 1: Crate `pathon-golden`

**Files:** `crates/pathon-golden/` (obraz, porównanie, PNG, magazyn referencji, odczyt tekstury, `gpu_or_skip`), `Cargo.toml` workspace (`png`, `pathon-golden`), `docs/crates/pathon-golden.md`, `docs/adr/0025-golden-images.md`.

- [x] **Step 1: Testy** (`tests/golden_store.rs`, `tests/readback_gpu.rs`) — tworzenie i dopasowanie referencji, brak referencji, test negatywny, progi, rozmiar, wariant, nazwy, diff, odczyt tekstury (null i GPU).
- [x] **Step 2: Implementacja** według ADR-0025.

### Task 2: Pierwszy golden test i CI

**Files:** `crates/pathon-core/tests/vertical_slice_gpu.rs` (`unlit_cube_matches_its_golden_image`), `tests/golden/unlit_cube.png`, `.github/workflows/ci.yml` (zadanie `golden`: lavapipe, WARP, macOS; artefakty przy porażce; `PATHON_REQUIRE_GPU` w zadaniu `gpu`), `docs/plan-implementacji.md`.

- [x] **Step 1: Test wycinka z M2-18** czyta cel offscreen i woła `assert_golden("unlit_cube", …, Tolerance::cross_backend())`.
- [x] **Step 2: Referencja** wygenerowana `PATHON_UPDATE_GOLDEN=1` i obejrzana.
- [x] **Step 3: CI** — zadanie macierzowe, artefakty, `--nocapture` żeby pominięcie było widoczne.
- [ ] **Step 4: Przebieg CI** (robi orkiestrator po push): zgodność lavapipe / WARP / Metal z referencją, ewentualny wariant w ADR-0025.
