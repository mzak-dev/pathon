# Warunki wstępne M4-04 / M4-06 w RHI — plan wdrożenia

**Cel:** IBL (M4-06) dostaje widoki sRGB i cube / tablice w `TextureDesc`, a bindless (M4-04, PAT-119) dowód na GPU, że `binding_array` działa z indeksem różnym dla wątków i pikseli.

**Spec:** aneks w [ADR-0020](../../adr/0020-kontrakty-rhi-types.md), [spike-ash.md](../../spike-ash.md) #4, [ADR-0017](../../adr/0017-tablice-bindingow.md).

## Ograniczenia globalne

- Nowe pola ze wsteczną zgodnością (konstruktory `new_2d` / `new_cube`, `Default` widoku); literały w workspace poprawione.
- Bez zmian w `pathon-shader`, nowych crate'ach i `ci.yml`. Bez bias / stencil / polygon-mode / alpha-to-coverage (przyjdą z pierwszym użytkownikiem).

## Review Focus

1. `rhi-null` odrzuca: cube bez 6 warstw lub niekwadratowy (R0109), `view_formats` spoza rodziny formatu (R0109), format widoku spoza `view_formats` (R0109), widok poza mipami i warstwami (R0112).
2. Domyślne `layer_count` zgodne z wgpu (`D2` = 1, `Cube` = 6, tablice = reszta), żeby null nie był łagodniejszy ani ostrzejszy niż backend na tych samych danych.
3. Testy GPU: kolory ścian cube'a trafiają w kierunki ±X/±Y/±Z; widok sRGB koduje przy zapisie i dekoduje przy odczycie; indeks nie-jednolity w compute i we fragmencie.

---

### Task 1: Widoki i `view_formats`

**Files:** `crates/pathon-rhi-types/src/{desc,format}.rs`, `crates/pathon-rhi-null/src/{view_rules,device}.rs`, `crates/pathon-rhi-wgpu/src/device.rs`; testy: `crates/pathon-rhi-null/tests/texture_views.rs`, `crates/pathon-rhi/tests/texture_views_gpu.rs`; docs: `docs/crates/pathon-rhi-types.md`, `docs/diag/R0109.md`, `R0112.md`, aneks ADR-0020.

- [x] **Step 1: Testy RED** (`rhi-null`), potem implementacja walidacji.
- [x] **Step 2: Test GPU** cube i sRGB.

### Task 2: Indeksowanie nie-jednolite (PAT-119)

**Files:** `crates/pathon-rhi/tests/binding_array_gpu.rs`, testy konwersji w `caps.rs` i `convert.rs`.

- [x] **Step 1:** bit `Features::NON_UNIFORM_INDEXING` już istnieje i jest zmapowany; dodane testy konwersji i profilu.
- [x] **Step 2:** testy GPU compute i fragment z indeksem `(i * 7) % 64`.
