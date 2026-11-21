# M2-24: pipeline cache — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** host podaje blob pipeline cache przy tworzeniu urządzenia i odbiera zaktualizowany blob; drugi start na Vulkanie korzysta z danych pierwszego.

**Spec:** [ADR-0019](../../adr/0019-pipeline-cache-i-zrodlo-shaderow.md); M2-24 w [planie](../../plan-implementacji.md). Decyzje o źródle shaderów i polach klucza są w ADR i nie zmieniają kodu w tym tasku.

## Ograniczenia globalne

- Brak nowych zależności; wszystkie testy (także GPU) zielone; wasm check z CI przechodzi.
- Jedyny nowy `unsafe` w API: `PipelineCacheData::from_bytes` z sekcją `# Safety`; każdy blok `unsafe` z `// SAFETY:`.
- Kod samokomentujący; docs po polsku.

## Review Focus

1. Blob z obcego adaptera albo uszkodzony → urządzenie startuje, potoki działają (bez paniki i bez błędu).
2. `DeviceRequest` z blobem bez włączonego `PIPELINE_CACHE` → blob ignorowany, urządzenie działa.
3. Każdy potok (render i compute) tworzony przez backend wgpu dostaje cache, gdy istnieje.

---

### Task 1: Pipeline cache

**Files:** `crates/pathon-rhi-types/src/device.rs` (+ eksport z `lib.rs`); `crates/pathon-rhi-wgpu/src/device.rs` (tworzenie cache w obu ścieżkach tworzenia urządzenia, `cache` w opisach potoków, getter); `crates/pathon-rhi-null/src/device.rs`; `crates/pathon-rhi/src/*.rs` (fasada `Device`); literały `DeviceRequest` w workspace (jeśli nie używają `..Default::default()`); testy: `crates/pathon-rhi/tests/` (feature `gpu-tests`); docs: `docs/crates/pathon-rhi-types.md`, `docs/crates/pathon-rhi-wgpu.md` (jeśli istnieje), `docs/plan-implementacji.md` (odhacz M2-24 ze stanem).

**Interfaces:**
- `pub struct PipelineCacheData(Vec<u8>)` z `Clone`, `Debug` (długość, nie bajty), `as_bytes(&self) -> &[u8]`, `unsafe fn from_bytes(bytes: Vec<u8>) -> Self`.
- `DeviceRequest::pipeline_cache: Option<PipelineCacheData>`.
- `DeviceApi::pipeline_cache_data(&self) -> Option<PipelineCacheData>`.

- [ ] **Step 1: Testy RED:**
  - GPU (Vulkan, `Features::PIPELINE_CACHE` jako wymagana; test pomija się czytelnie, gdy brak adaptera Vulkan z tą funkcją): urządzenie A tworzy potok render i compute, `pipeline_cache_data()` zwraca niepusty blob; urządzenie B dostaje ten blob w `DeviceRequest`, tworzy te same potoki, rysuje/dispatchuje poprawnie i też zwraca niepusty blob.
  - GPU: blob z losowych bajtów (`unsafe from_bytes`) → urządzenie startuje, potok działa.
  - GPU albo null: blob bez włączonego `PIPELINE_CACHE` → urządzenie startuje, `pipeline_cache_data()` = `None`.
  - null: `pipeline_cache_data()` = `None`.
- [ ] **Step 2: Implementacja** według ADR-0019.
- [ ] **Step 3: Uruchom** `cargo test --workspace --all-features`, clippy `-D warnings`, wasm check, deps-check.
- [ ] **Step 4: Commit** `feat(rhi)!: load and save the pipeline cache blob (ADR-0019, M2-24)`
