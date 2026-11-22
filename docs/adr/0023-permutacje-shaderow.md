# ADR-0023: Permutacje shaderów i klucze potoków

**Status:** Przyjęta
**Data:** 2026-11-22

## Kontekst

M3-03 rozdziela **klucz shadera** (flagi materiału, hooki paczek, profil) od **klucza potoku** (layout wierzchołków, formaty celów, MSAA, rodzaj widoku). Shadery składa `pathon-shader` przez wesl (ADR-0021), refleksja jest w tym samym crate'cie (M3-02). Opisy potoków mają `Eq + Hash` (ADR-0018, ADR-0019), więc mogą być kluczem wprost. Paczki zależą tylko od `pathon-api`, a rdzeń nie ciągnie wesl (+50 crate'ów, ADR-0021).

## Decyzja

- **Permutacje to flagi WESL `@if(flaga)`** (kompilacja warunkowa wesl). Klucz shadera: `ShaderKey { root, features }` — nazwa zarejestrowanego shadera głównego i **zbiór włączonych flag** (posortowany; flaga wyłączona = nieobecna). Flaga, której nikt nie włączył, jest wyłączona. Wartości liczbowe nie są flagami: idą przez overrides (ADR-0018) i należą do klucza potoku.
- Profil (Web / Desktop / High) i hooki paczek to zwykłe flagi wybrane przez paczkę (np. `web`, `shadows`); rdzeń ich nie interpretuje.
- **`ShaderLibrary`** w `pathon-shader`: moduły (przez `ShaderComposer`), shadery główne rejestrowane pod nazwą i kesz `ShaderKey → ShaderModuleId` na urządzeniu. Ta sama permutacja składa się i tworzy moduł raz.
- **Klucz potoku = opis potoku** (`RenderPipelineDesc` / `ComputePipelineDesc`) z modułem z `ShaderLibrary`, po normalizacji: `constants` posortowane po nazwie. `PipelineCache` w `pathon-shader` trzyma `opis → RenderPipelineId / ComputePipelineId`. Permutacja wchodzi do klucza potoku przez `ShaderModuleId`, więc dwa klucze shadera nie dają tego samego potoku.
- **Granica paczek:** paczka może zależeć bezpośrednio od `pathon-shader` (obok `pathon-api`); xtask (`pack-boundary`) dopuszcza ten crate. Rdzeń nie zależy od `pathon-shader` — składanie, kesz i (w M3-04) kompilacja w tle należą do paczek, a moduły współdzielone między paczkami trzymają one w `Shared`.
- Unieważnianie kesza po zmianie modułu (hot-reload) zostaje na M3-06.

## Konsekwencje

- Rdzeń zostaje bez wesl i naga (bazowa liczba crate'ów rdzenia bez zmian); paczka ze shaderami płaci za `pathon-shader`.
- `pathon-shader` jest drugim crate'em kontraktu paczek obok `pathon-api`, więc M5-07 (`cargo-semver-checks`) musi objąć oba.
- Literówka w nazwie flagi po cichu wyłącza gałąź `@if` — test paczki musi sprawdzać permutacje, których używa.
- Kesz rośnie z każdą permutacją; limit i sprzątanie przyjdą z pomiarem (M3-04, M4-17).
