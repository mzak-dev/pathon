# ADR-0041: crate'y `pack-lib` — wspólne biblioteki paczek

- **Status:** przyjęty (22.11.2026)
- **Dotyczy:** reguła `pack-boundary` w `xtask deps-check`, M4-03…M4-06, M4-05

## Kontekst

Reguła z M0-02 i ADR-0023 mówi, że paczka zależy z crate'ów pathon tylko od `pathon-api` i `pathon-shader`. Przy M4-02…M4-04 wyszło, że to za mało: `pathon-pbr` musi złożyć shader z modułów materiału (`pathon-material`), tekstur (`pathon-bindless`), IBL (`pathon-ibl`) i geometrii (`pathon-mesh`, `pathon-scene`). Paczki te były do tej pory zamknięte w sobie: scena mapuje uchwyty przez `RawHandle`, a hook materiału jest tylko napisem WGSL. Typy musiałyby się spotkać w jednym miejscu, a tym miejscem jest paczka-konsument.

## Decyzja

Nowy rodzaj crate'u: `[package.metadata.pathon] kind = "pack-lib"`.

- `pack-lib` to biblioteka, od której **mogą zależeć inne paczki** (typy, moduły WGSL, kod pomocniczy). Może zawierać własne `RenderFeature`.
- Sam `pack-lib` podlega regule `pack-boundary` jak paczka: zależy od `pathon-api`, `pathon-shader` i innych `pack-lib`.
- Paczka (`kind = "pack"`, `vendor`, albo katalog `packs/`) może zależeć od `pathon-api`, `pathon-shader` i dowolnego `pack-lib`. **Nie** może zależeć od innej paczki, która nie jest `pack-lib` (np. `pathon-post`, `pathon-test`).
- Cykle wyklucza cargo.
- Rdzeń (`pathon-core` i reszta domknięcia) nie zależy od żadnego `pack-lib`; baseline domknięcia rdzenia się nie zmienia.

Oznaczone jako `pack-lib` od teraz: `pathon-mesh`, `pathon-scene`, `pathon-material`, `pathon-bindless`, `pathon-ibl`, `pathon-texture`.

## Konsekwencje

- Zależność jest jawna w `Cargo.toml` i sprawdzana: dwa testy w `xtask/src/rules.rs` (paczka może użyć `pack-lib`, nie może innej paczki; `pack-lib` sam przestrzega granicy).
- Wymiana implementacji (np. innego materiału) idzie przez hooki WGSL i `Shared`, a nie przez zmianę zależności.
- Zasada „nowa paczka zwykła nie wymaga zmian w `pathon-core`” zostaje spełniona.
- Ryzyko: łatwo rozrosnąć sieć zależności między paczkami. Nowy `pack-lib` wymaga uzasadnienia w PR; paczki funkcjonalne (TAA, SSAO, FXAA, …) pozostają `kind = "pack"` i nie zależą od siebie.
