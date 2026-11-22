# M3-02: refleksja layoutów bind groupów — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** z WGSL (złożonego przez `ShaderComposer`) i listy entry pointów powstają opisy layoutów bind groupów w typach pathon, równe ręcznie zapisanym.

**Spec:** [ADR-0006](../../adr/0006-wejscie-shaderow.md) (refleksję liczy `pathon-shader` z naga), [ADR-0017](../../adr/0017-tablice-bindingow.md) (tablice), [ADR-0021](../../adr/0021-importy-shaderow-wesl.md); M3-02 w [planie](../../plan-implementacji.md).

## Decyzje (wiążące dla tasku)

- `pathon-shader` dostaje zależności `naga` 30 (tylko `wgsl-in`, bez domyślnych cech) i `pathon-rhi-types`. `naga` jest wpięty przez `PINNED_DEPENDENCIES` tylko w `pathon-shader` (ADR-0006: naga poza API RHI). Popraw zdanie „nie zależy od RHI, wgpu ani naga” w ADR-0021 i `docs/crates/pathon-shader.md`: zależy od `pathon-rhi-types` i naga, nie od backendów ani wgpu.
- **Zakres:** binding wchodzi do layoutu, gdy używa go statycznie (naga `ModuleInfo`, `GlobalUse` niepuste) co najmniej jeden z podanych entry pointów; `visibility` to suma etapów tych entry pointów. Nieużywane bindingi są pomijane (jak automatyczny layout wgpu). Wynik to `Vec<BindGroupLayoutDesc>` indeksowany numerem grupy, do najwyższej używanej grupy; grupy bez bindingów są puste; wpisy posortowane po `binding`; `label` = `"reflected"`.
- **Mapowanie:** `uniform` → `UniformBuffer`; `storage, read` → `StorageBuffer { ReadOnly }`, `read_write` → `ReadWrite`; tekstura próbkowana `f32` → `Float` (filtrowalna — format, który filtrowania nie wspiera, poprawia wołający), `i32` → `Sint`, `u32` → `Uint`, `f32` multisampled → `UnfilterableFloat` (wgpu odrzuca filtrowalne multisampled), głębia → `Depth`, `multisampled` z typu; tekstura storage → `StorageTexture` z formatem i dostępem (`read` → `ReadOnly`, `write` → `WriteOnly`, `read_write` → `ReadWrite`); `sampler` → `Filtering`, `sampler_comparison` → `Comparison`; wymiar z typu tekstury (`D1`, `D2`, `D2Array`, `Cube`, `CubeArray`, `D3`); `binding_array<T, N>` → `count = Some(N)` i typ z `T`. Zmienne w przestrzeni `immediate` nie są bindingami i są pomijane.
- **Błędy** (`ShaderError`, nowe warianty): błąd parsowania / walidacji naga (walidacja z `Capabilities::all()`), nieznany entry point, binding bez odpowiednika w RHI (np. acceleration structure, tablica bez stałego rozmiaru, format storage, którego nie ma w `Format`), dwa różne bindingi pod tym samym `(group, binding)` użyte przez podane entry pointy. Bez paniki.

## Ograniczenia globalne

- Brak innych nowych zależności; rdzeń nie zależy od `pathon-shader` (bazowa liczba crate'ów rdzenia bez zmian); wasm check z `pathon-shader` przechodzi.
- Kod samokomentujący, docs po polsku.

## Review Focus

1. Test „Gotowe gdy”: layout z refleksji równa się ręcznie zapisanemu dla shadera render (vertex + fragment) z uniformem, teksturą, samplerem, storage bufferem, teksturą storage i `binding_array` w dwóch grupach.
2. `visibility` i pomijanie bindingów zależą od podanych entry pointów (ten sam moduł, różne zestawy → różne layouty).
3. Każdy niewspierany przypadek to `Err`, nie panika; konflikt `(group, binding)` wykryty.

---

### Task 1: Refleksja

**Files:** `crates/pathon-shader/Cargo.toml`, `src/lib.rs` (+ `src/reflect.rs`); root `Cargo.toml` (`[workspace.dependencies]` `naga`); `xtask/src/rules.rs` (pin `naga` → `pathon-shader`); testy: `crates/pathon-shader/tests/reflect.rs`; test GPU (`gpu-tests`): layout z refleksji tworzy bind group layout i potok na wgpu, który działa (np. compute z uniformem i storage bufferem); docs: `docs/crates/pathon-shader.md`, ADR-0021 (zdanie o zależnościach), `docs/plan-implementacji.md` (odhacz M3-02 ze stanem).

**Interfaces:**
- `pub fn reflect_bind_groups(wgsl: &str, entry_points: &[&str]) -> Result<Vec<BindGroupLayoutDesc>, ShaderError>`.

- [ ] **Step 1: Testy RED:** przypadki z Review Focus; dodatkowo: compute z `read` i `read_write` storage; tekstura głębi z `sampler_comparison`; `texture_multisampled_2d`; `texture_storage_2d<rgba8unorm, write>`; `texture_cube_array`; `var<immediate>` pominięty; nieznany entry point; konflikt `(group, binding)`; acceleration structure (WGSL `acceleration_structure` z `enable wgpu_ray_query;`) → błąd; shader złożony przez `ShaderComposer` z bindingami w module.
- [ ] **Step 2: Implementacja** według decyzji wyżej.
- [ ] **Step 3: Uruchom** `cargo test --workspace --all-features`, clippy `-D warnings`, wasm check, deps-check.
- [ ] **Step 4: Commit** `feat(shader): reflect bind group layouts from WGSL with naga (M3-02)`
