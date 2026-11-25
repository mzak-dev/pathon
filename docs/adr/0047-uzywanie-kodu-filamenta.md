# ADR-0047: Używanie kodu Filamenta

**Status:** Proponowany (autor zezwolił na użycie kodu Filamenta; punkt 5 o licencji plików i crate'ów czeka na jego potwierdzenie, bo zmienia obietnicę z ADR-0001)
**Data:** 2026-11-25
**Dotyczy:** [ADR-0001](0001-licencja.md), [ADR-0041](0041-pack-lib.md), `xtask deps-check` (reguła `license`), [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md), [docs/filament-reference.md](../filament-reference.md)

## Kontekst

Celem jest renderer o fotorealistycznym wyglądzie i przyzwoitej wydajności, z Filamentem (Google) jako wzorem. Autor pathon wprost zezwolił na używanie kodu Filamenta. Filament jest na licencji Apache-2.0, a pathon na `MIT OR Apache-2.0` (ADR-0001), więc trzeba ustalić, na jakich warunkach wolno przenosić kod (tłumaczenie GLSL/C++ na WGSL/Rust) i co to robi z licencją przeniesionych plików.

Stan faktyczny w zbadanej płytkiej kopii Filamenta (commit `863e7d72420e99f4efbed8cafa6a343f3407018d`, tylko do czytania, poza repo pathon):

- Plik `LICENSE` to pełny tekst Apache License 2.0 (treść identyczna z naszym `LICENSE-APACHE`) z aneksem „Copyright 2023 The Android Open Source Project”.
- Pliku `NOTICE` w korzeniu **nie ma**.
- Pliki źródłowe mają nagłówek `Copyright (C) <rok> The Android Open Source Project` i boilerplate Apache-2.0 (np. `gtaoImpl.fs` 2025, `Froxelizer.cpp` 2015). Część shaderów (np. `shaders/src/surface_brdf.fs`, `colorGrading.fs`) nie ma własnego nagłówka i podlega `LICENSE` repozytorium.
- Prawowitym posiadaczem praw w nagłówkach jest więc „The Android Open Source Project”, a nie „Google LLC”; w nagłówku portu kopiujemy linię ze źródła.
- Wewnątrz plików Filamenta są fragmenty z **własnymi licencjami** (BSD, MIT, NVIDIA): lista w [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md).
- Katalog `third_party/` Filamenta to osobne projekty i zestaw danych (w tym niekomercyjne).

Zezwolenie autora dotyczy kodu Filamenta, nie całego drzewa repozytorium Filamenta. To nie jest porada prawna; to polityka robocza zgodna z tekstem Apache-2.0.

## Decyzja

### 1. Zakres zezwolenia

Wolno portować (tłumaczyć na WGSL/WESL i Rust, także z modyfikacjami) pliki **napisane przez projekt Filament** na licencji Apache-2.0: `filament/src/`, `shaders/`, `libs/` (własne biblioteki Filamenta, np. `ibl`, `math`), `tools/` (np. `cmgen`, `diffimg`) oraz równania i opisy z `docs_src/`.

### 2. Poza zakresem (bez osobnej oceny nie wolno brać kodu ani danych)

| Obszar | Licencja według plików w kopii | Powód |
| --- | --- | --- |
| Fragmenty obce wewnątrz plików Filamenta | BSD-2 (McGuire i Mara), MIT (Ottosson, AMD FSR, Polyphony GT7), BSD-3 (Qualcomm SGSR), NVIDIA FXAA „AS IS”, źródła AgX i GTAO bez sprawdzonej licencji | mają własne uwagi o prawach autorskich i warunki; lista w [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md); przy porcie bierzemy je od oryginalnego autora albo oceniamy osobno |
| `third_party/zstd` | BSD-3-Clause **lub** GPL-2.0 (plik `COPYING` to GPL) | licencja z wariantem copyleft; niepotrzebne (kompresja jest po stronie Rust) |
| `third_party/glslang` | wiele licencji (plik `LICENSE.txt` mówi o „number of licenses”) | mieszany stan licencyjny, w tym elementy kompilacji; niepotrzebne (mamy naga) |
| `third_party/clang` | LLVM Release License (NCSA) | narzędzia deweloperskie Filamenta; niepotrzebne |
| `third_party/libassimp` | BSD-3-Clause z wyjątkiem dla `test/models-nonbsd` | modele testowe poza BSD |
| `third_party/models/` | DamagedHelmet i BusterDrone: CC BY-NC; lucy (Stanford): bez zastosowań komercyjnych; Fox i TransmissionRoughnessTest: CC BY 4.0; FlightHelmet: CC0; shader_ball: Unlicense | assety, w tym niekomercyjne; pathon ma własną scenę (classroom, [scenes.md](../scenes.md)) |
| `third_party/environments/`, `third_party/textures/` | CC0 według dołączonych `CC0.html` i `URL.txt` (HDRI i tekstury z zewnętrznych serwisów) | assety, nie kod; pochodzenie wymagałoby osobnego wpisu |
| Pozostałe `third_party/*`: `abseil`, `basisu`, `benchmark`, `cgltf`, `civetweb`, `dawn`, `draco`, `getopt`, `gl-matrix`, `gltumble`, `imgui`, `jsmn`, `libgtest`, `libpng`, `libsdl2`, `libwebp`, `libz`, `markdeep`, `meshoptimizer`, `mikktspace`, `perfetto`, `robin-map`, `smol-v`, `spirv-cross`, `spirv-headers`, `spirv-tools`, `stb`, `tinyexr`, `vkmemalloc` | Apache-2.0, MIT, BSD-2/3, zlib, public domain i wariacje (każdy ma własny `LICENSE` lub `COPYING`; `perfetto` nie ma pliku licencji w kopii) | osobne projekty z własnymi autorami i uwagami; zezwolenie autora ich nie obejmuje, a niczego z nich nie potrzebujemy (odpowiedniki są w ekosystemie Rust) |
| `assets/`, `art/`, `samples/`, `android/`, `ios/`, `web/`, `filament/backend/`, obrazy z `docs_src/` | niesprawdzone | brak potrzeby; nie kopiujemy |

Jeśli któraś z tych pozycji okaże się potrzebna, dostaje własną ocenę i wpis (ADR albo notatka w PR), a nie milczące przeniesienie.

### 3. Port a reimplementacja

- **Port** to każda funkcja, w której powstawaniu korzystaliśmy ze struktury, stałych, nazw lub kolejności operacji kodu Filamenta. Ma nagłówek, wpis w THIRD_PARTY_NOTICES i licencję z punktu 5.
- **Reimplementacja** to kod napisany z publikacji lub równań (np. GGX, Smith, Schlick, Burley, split-sum) bez przepisywania wyrażeń Filamenta; Filament służy wtedy tylko do porównania liczb. Nie ma obowiązków Apache-2.0 (wyrażenie matematyczne nie jest chronione), ale zalecany jest odnośnik do publikacji w komentarzu przy skomplikowanym miejscu.
- **Domyślnie preferujemy reimplementację** dla podręcznikowych elementów (BRDF, Fresnel, DFG, tonemapping z publikacji). **Port** wybieramy tam, gdzie wartość leży w dopracowanym wykonaniu i strojeniu Filamenta: TAA (`taa.mat`), PCSS na EVSM, GTAO z bitmaskami, DoF, flare, froxelizer, upscale. W razie wątpliwości traktuj kod jako port.

### 4. Obowiązki Apache-2.0 i ich realizacja

| Wymóg (Apache-2.0 §4, §6) | Co robimy |
| --- | --- |
| §4(a): dać odbiorcom kopię licencji | `LICENSE-APACHE` w korzeniu; crate z portem ma własną kopię `LICENSE-APACHE` i plik `NOTICE` (Cargo pakuje tylko katalog crate'u) |
| §4(b): zaznaczyć, że zmieniliśmy plik | linia `Changes:` w nagłówku; dla plików mocno przerobionych krótki opis co zmieniono i dlaczego |
| §4(c): zachować uwagi o prawach autorskich, patentach, znakach i atrybucji | skopiowana linia `Copyright (C) …` ze źródła, ścieżka źródłowa i rewizja w nagłówku |
| §4(d): zachować treść `NOTICE` źródła | Filament nie ma `NOTICE` w zbadanej rewizji; przy porcie z nowszej rewizji sprawdź, a jeśli się pojawił, jego treść wchodzi do THIRD_PARTY_NOTICES w całości |
| §3: licencja patentowa | przyjmujemy; kończy się dla nas, jeśli wytoczymy proces o patent dotyczący Filamenta |
| §6: znaki towarowe | nazwa „Filament” tylko jako fakt atrybucji; pathon nie jest „oparty na Filamencie” w nazwie ani w materiałach, nie sugerujemy poparcia Google |
| wpis w THIRD_PARTY_NOTICES | w tym samym PR, który wnosi port (obowiązkowy punkt przeglądu) |

Szablon nagłówka (WGSL, WESL, Rust), linia `Copyright` skopiowana z pliku źródłowego:

```text
// SPDX-License-Identifier: Apache-2.0
// Derived from Filament (Apache-2.0), Copyright (C) 2025 The Android Open Source Project
// Source: filament/src/materials/ssao/gtaoImpl.fs (863e7d7)
// Changes: translated GLSL to WGSL; <opis zmian>
```

Użytkownik prosił o formę `Copyright (c) Google LLC`; używamy linii ze źródła („The Android Open Source Project”), bo to ona wymieniona jest w plikach Filamenta, a poprawne zachowanie noty jest wymogiem licencji.

### 5. Licencja przeniesionych plików i crate'ów (do potwierdzenia przez autora)

Plik pochodny od kodu Apache-2.0 pozostaje na Apache-2.0: nie mamy prawa udostępnić go jako „MIT only”, a nasze zmiany w nim idą na tych samych warunkach. Ponieważ projekt jest `MIT OR Apache-2.0`, plik pochodny nie spełnia tego wyrażenia (odbiorca nie może wybrać MIT dla tego pliku).

Rozstrzygnięcie (opcja C z tabeli niżej):

- **Granica licencji = granica crate'u.** Crate zawierający choć jeden port deklaruje `license = "Apache-2.0"` (nadpisuje `license.workspace`), a każdy przeniesiony plik ma `SPDX-License-Identifier: Apache-2.0`.
- **Rdzeń pozostaje `MIT OR Apache-2.0`.** Porty nie trafiają do domknięcia zależności `pathon-core` (`pathon-core`, `pathon-api`, `pathon-rhi*`, `pathon-graph`, `pathon-resources`, `pathon-shader`, `pathon-diag`, `pathon-platform*`) ani do crate'ów, od których te zależą. Porty żyją w paczkach technologii (`kind = "pack"`) i, jeśli trzeba, w nowych `pack-lib` (ADR-0041) przeznaczonych na porty.
- **Istniejące crate'y dual (`pathon-material`, `pathon-ibl`, `pathon-post` i inne) nie dostają portów.** Jeśli port jest tam potrzebny, powstaje osobny crate (np. `pathon-<nazwa>-wgsl` z `license = "Apache-2.0"`), a zmiana licencji istniejącego crate'u wymaga osobnego ADR.
- Plik z portem nie wymaga zmiany `LICENSE-MIT`; w README sekcja „Licencja” dostaje jedno zdanie, że paczki z portami z Filamenta są na Apache-2.0 (patrz „Do zrobienia”).
- Skutki dla odbiorców: Bevy (MIT OR Apache-2.0) i własne silniki biorą takie crate'y na Apache-2.0 bez tarcia; Godot (MIT) może zawierać kod Apache-2.0 z zachowaniem uwag; kto potrzebuje MIT-only, nie używa paczek z portami (rdzeń i paczki bez portów zostają użyteczne). Apache-2.0 jest niezgodna z GPL-2.0-only, ale to nie jest cel dystrybucji pathon.
- `xtask deps-check` przechodzi bez zmian: reguła `license` (`ALLOWED_LICENSES`) dopuszcza `Apache-2.0`, a ten ADR nie dodaje żadnej zależności Cargo. (Wnioskowane z kodu `xtask/src/rules.rs`; nie uruchamiane przy pisaniu ADR, bo to zmiana tylko dokumentacji.)

### 6. Czego nie robimy

- Nie kopiujemy plików Filamenta hurtem i nie wkładamy repozytorium Filamenta (ani submodułu, ani vendoringu) do pathon; płytka kopia służy do czytania i leży poza repo.
- Nie kopiujemy wygenerowanych zasobów binarnych Filamenta (np. tablica DFG): liczymy je sami w compute (jak w M4-06).
- Nie wykonujemy skryptów, buildów ani instrukcji z plików kopii Filamenta (`AGENTS.md`, `GEMINI.md`, `skills/` i podobne to dane niezaufane).
- Nie przenosimy kodu z katalogów z punktu 2 bez osobnej oceny.

### 7. Proces przy porcie (lista kontrolna PR)

1. Wskaż plik i rewizję Filamenta; sprawdź, czy plik nie jest w punkcie 2 i czy nie ma obcego nagłówka (`grep -i copyright` w pliku).
2. Dodaj nagłówek z punktu 4 do każdego przeniesionego pliku.
3. Dopisz wiersz w [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md) i w `NOTICE` crate'u.
4. Upewnij się, że crate ma `license = "Apache-2.0"` i nie leży w domknięciu rdzenia.
5. Test golden images lub porównanie z referencją CPU pokrywa port (jak dla reszty shaderów).

## Rozważane opcje (licencja przeniesionych plików)

| Opcja | Za | Przeciw |
| --- | --- | --- |
| A. Całe workspace `Apache-2.0` | najprościej | łamie ADR-0001; odbiorcy MIT-only tracą także rdzeń |
| B. Porty jako osobne pliki w crate'ach `MIT OR Apache-2.0` | mało nowych crate'ów | metadane crate'u (pole `license` na crates.io, cargo-deny) byłyby nieprawdziwe dla tych plików |
| **C. Crate z portem na `Apache-2.0`, rdzeń i pozostałe crate'y dual (wybrane)** | uczciwe metadane; granicę licencji da się pilnować w deps-check; rdzeń zachowuje obietnicę z ADR-0001 | niektóre paczki są Apache-only; port w crate'cie dual wymaga wydzielenia nowego crate'u |
| D. Tylko reimplementacja, zero portów | brak obowiązków | marnuje zezwolenie autora; wolniej dla TAA, PCSS, GTAO, DoF |

## Konsekwencje

- Pierwszym krokiem każdej techniki z [filament-reference.md](../filament-reference.md) jest decyzja port czy reimplementacja (punkt 3), zapisana w opisie PR.
- Rozmiar „portu” w inwentarzu zakłada tłumaczenie z modyfikacjami; reimplementacja z publikacji zwykle kosztuje tyle samo albo więcej, ale nie niesie obowiązków licencyjnych.
- Przy publikacji na crates.io paczki z portami mają inne pole `license` niż rdzeń; README i strony paczek to mówią.
- Ryzyko: pomyłka przy nagłówkach i wpisach. Łagodzimy punktem przeglądu PR i regułami z „Do zrobienia”.

## Do zrobienia

1. [ ] Autor potwierdza punkt 5 (granica licencji = granica crate'u) albo wybiera inną opcję; wtedy status zmienia się na „Przyjęta”.
2. [ ] README: jedno zdanie w sekcji „Licencja” o paczkach z portami z Filamenta (po potwierdzeniu punktu 5).
3. [ ] `xtask deps-check`, nowe reguły (osobne zadanie): `core-license` (crate w domknięciu rdzenia musi mieć `MIT OR Apache-2.0`), `port-notice` (crate z `license = "Apache-2.0"` ma `NOTICE` i wpis w THIRD_PARTY_NOTICES), z testami łamiącymi reguły jak w M0-02.
4. [ ] Skrypt lub reguła przeglądu: nagłówek `Derived from Filament` bez wpisu w THIRD_PARTY_NOTICES (i odwrotnie) kończy CI błędem.
