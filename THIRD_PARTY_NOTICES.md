# Third-party notices

Ten plik wymienia kod i dane przeniesione do pathon z cudzych projektów oraz ich licencje. Zasady: [ADR-0047](docs/adr/0047-uzywanie-kodu-filamenta.md) (Filament) i [ADR-0001](docs/adr/0001-licencja.md) (licencja projektu). Zależności Cargo (wgpu, naga, wesl i inne) nie są tu wypisane: ich licencje pilnuje `cargo xtask deps-check` (reguła `license`).

Każdy przeniesiony plik ma też własny nagłówek (szablon niżej), a crate z takim plikiem ma plik `NOTICE` ze swoimi wpisami. Wpis w tym pliku jest obowiązkowy w tym samym PR, który wnosi port.

## Przeniesione pliki

Na dziś nie przeniesiono żadnego pliku: w `crates/`, `examples/`, `tools/` i `xtask/` nie ma śladu po Filamencie (sprawdzone grepem 25.11.2026; jedyna wzmianka to zdanie o przybliżeniu clearcoat w [ADR-0030](docs/adr/0030-material-standardowy-i-hooki-wgsl.md), bez kodu).

| Plik w pathon | Źródło (projekt, ścieżka, rewizja) | Licencja źródła | Zmiany | Licencja pliku w pathon |
| --- | --- | --- | --- | --- |
| *(brak)* | | | | |

### Szablon wpisu

Dopisz wiersz do tabeli powyżej, a pod tabelą sekcję z pełnym tekstem uwag o prawach autorskich ze źródła, jeśli źródło ma własną (np. BSD, MIT):

```markdown
| `crates/pathon-ssao/wgsl/gtao.wgsl` | Filament, `filament/src/materials/ssao/gtaoImpl.fs`, 863e7d7 | Apache-2.0, Copyright (C) 2025 The Android Open Source Project | GLSL → WGSL, pobieranie głębi przez `textureLoad`, usunięty wariant bitmasek | Apache-2.0 |
```

Nagłówek w pliku (WGSL / WESL / Rust; linia `Copyright` jest **skopiowana z pliku źródłowego**, a nie wymyślona):

```text
// SPDX-License-Identifier: Apache-2.0
// Derived from Filament (Apache-2.0), Copyright (C) 2025 The Android Open Source Project
// Source: filament/src/materials/ssao/gtaoImpl.fs (863e7d7)
// Changes: translated GLSL to WGSL; <opis zmian>
```

Gdy plik źródłowy nie ma własnego nagłówka (np. `shaders/src/surface_brdf.fs`), druga linia brzmi: `Derived from Filament (Apache-2.0), see LICENSE of the Filament repository`.

## Filament

- Projekt: <https://github.com/google/filament>, licencja Apache-2.0.
- Zbadana rewizja (płytka kopia tylko do czytania, poza repo pathon): gałąź `main`, commit `863e7d72420e99f4efbed8cafa6a343f3407018d` (RELEASE_NOTES.md: nagłówek v1.81.1).
- Plik `NOTICE` w korzeniu tej rewizji **nie istnieje**. Przy każdym porcie z nowszej rewizji sprawdź, czy się nie pojawił; jeśli tak, jego treść trafia tutaj w całości (Apache-2.0 §4(d)).
- Pełny tekst Apache License 2.0 jest w [LICENSE-APACHE](LICENSE-APACHE) (treść identyczna z plikiem `LICENSE` Filamenta; Filament dodaje na końcu aneks z uwagą o prawach autorskich, przepisany niżej).

Uwaga o prawach autorskich z pliku `LICENSE` Filamenta (przepisana dosłownie):

```text
Copyright 2023 The Android Open Source Project

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```

Pliki źródłowe Filamenta noszą własne nagłówki w postaci `Copyright (C) <rok> The Android Open Source Project`; to ten wiersz (z rokiem z danego pliku) kopiujemy do nagłówka portu.

### Fragmenty obce wewnątrz plików Filamenta (poza zgodą na kod Filamenta)

Te fragmenty leżą w plikach Filamenta, ale mają **własne licencje** i nie wolno ich przenosić bez osobnej oceny i osobnego wpisu (ADR-0047, sekcja „Poza zakresem”):

| Fragment | Plik Filamenta | Autor i licencja według nagłówka w pliku |
| --- | --- | --- |
| Ślad promienia w przestrzeni ekranu (SSR) | `shaders/src/surface_light_reflections.fs` | Morgan McGuire, Michael Mara 2014, BSD 2-Clause |
| Przycinanie gamutu (OKLab) | `filament/src/ColorSpaceUtils.cpp` | Björn Ottosson 2021, MIT |
| FXAA 3.11 w wersji G3D | `filament/src/materials/antiAliasing/fxaa/fxaa.fs` | NVIDIA (Timothy Lottes) 2010–2011, własny tekst „AS IS”; G3D: Morgan McGuire, BSD |
| FSR 1 (EASU, RCAS, `ffx_a.h`) | `filament/src/materials/fsr/` | AMD 2021, MIT; fragmenty Michala Drobota 2014 |
| SGSR 1 | `filament/src/materials/sgsr/` | Qualcomm Innovation Center 2023, BSD 3-Clause |
| Tone mapper GT7 | `filament/src/ToneMapper.cpp` (sekcja GT7) | Polyphony Digital 2025, MIT |
| Tone mapper AgX (macierze i aproksymacja kontrastu) | `filament/src/ToneMapper.cpp` (sekcja AgX) | źródła: skrypt EaryChow (Blender), wpis na blogu iolite; licencje niesprawdzone |
| GTAO (struktura i szum) | `filament/src/materials/ssao/gtaoImpl.fs` | nagłówek wskazuje XeGTAO (Intel) i repozytorium Unity; licencje źródeł do sprawdzenia przy porcie |

Pełny wykaz katalogów `third_party/` Filamenta, z których nie wolno brać kodu ani danych, jest w ADR-0047.

## Inne źródła

Brak wpisów. Szablon dla kodu spoza Filamenta (np. FSR 1 z repozytorium AMD, MIT): ten sam wiersz tabeli, tekst licencji źródła pod tabelą w osobnej sekcji `###`.
