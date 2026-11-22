# M3-03: permutacje i klucze potoków — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** paczka prosi o permutację shadera kluczem (shader główny + flagi) i o potok opisem; każda unikalna permutacja i każdy unikalny potok powstaje raz, a różne klucze nigdy nie dają tego samego obiektu.

**Spec:** [ADR-0023](../../adr/0023-permutacje-shaderow.md), [ADR-0021](../../adr/0021-importy-shaderow-wesl.md), [ADR-0018](../../adr/0018-immediates-i-overrides.md); M3-03 w [planie](../../plan-implementacji.md).

## Ograniczenia globalne

- Wszystko w `pathon-shader`; brak nowych zależności; rdzeń nie zależy od `pathon-shader` (bazowa liczba crate'ów rdzenia bez zmian); wasm check przechodzi.
- Kod samokomentujący, docs po polsku.

## Review Focus

1. Testy kolizji: ta sama permutacja z flagami podanymi w innej kolejności albo z jawnie wyłączoną flagą → ten sam moduł (jedno złożenie); inne flagi → inny moduł; `constants` w innej kolejności → ten sam potok; różnica w formacie celu, `sample_count`, layoucie wierzchołków albo w module → inny potok.
2. `@if(flaga)` naprawdę zmienia wygenerowany WGSL (test na wyniku, nie tylko na kluczu).
3. Błędy (nieznany shader główny, błąd składania, błąd tworzenia modułu / potoku) wracają jako `Err`, a kesz nie zapamiętuje nieudanej próby.

---

### Task 1: Klucze i kesze

**Files:** `crates/pathon-shader/src/` (nowe moduły na `ShaderKey`, `ShaderLibrary`, `PipelineCache`; `ShaderComposer` dostaje flagi); `xtask/src/rules.rs` (`pack_boundary`: paczka może zależeć od `pathon-shader`; test reguły); testy: `crates/pathon-shader/tests/` (klucze i kesze na `rhi-null`; jeden test GPU: dwie permutacje z `@if` dają różne wyniki compute); docs: `docs/crates/pathon-shader.md`, `docs/plan-implementacji.md` (odhacz M3-03 ze stanem), strona xtask / zasad zależności w docs, jeśli opisuje `pack-boundary`.

**Interfaces:**
- `ShaderComposer::compose_with(&self, source: &str, file_path: &str, features: &ShaderFeatures) -> Result<String, ShaderError>` (obecne `compose` = bez flag).
- `pub struct ShaderFeatures` — posortowany zbiór włączonych flag (`Clone, PartialEq, Eq, Hash, Debug, Default`), `with(name)`, `set(name, enabled)`, iterator; nazwa flagi musi być identyfikatorem WGSL (inaczej `ShaderError`).
- `pub struct ShaderKey { pub root: String, pub features: ShaderFeatures }` (`Clone, PartialEq, Eq, Hash, Debug`).
- `pub struct ShaderLibrary` z `new()`, `add_module(path, source)`, `add_root(name, source)`, `module(&mut self, device: &dyn DeviceApi, key: &ShaderKey) -> Result<ShaderModuleId, ShaderError>` (błąd RHI opakowany w wariant `ShaderError`).
- `pub struct PipelineCache` z `render(&mut self, device, desc: &RenderPipelineDesc) -> Result<RenderPipelineId, RhiError>` i `compute(...)`; normalizuje `constants` przed wyszukaniem.

- [ ] **Step 1: Testy RED** z Review Focus (na `rhi-null`, liczenie utworzeń przez porównanie zwróconych id) + test GPU z `@if`.
- [ ] **Step 2: Implementacja** według ADR-0023.
- [ ] **Step 3: Uruchom** `cargo test --workspace --all-features`, clippy `-D warnings`, fmt, wasm check, deps-check.
- [ ] **Step 4: Commit** `feat(shader): add shader permutation keys and pipeline caches (ADR-0023, M3-03)`
