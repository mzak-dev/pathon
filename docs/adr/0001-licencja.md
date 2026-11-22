# ADR-0001: Licencja

**Status:** Przyjęta
**Data:** 2026-11-20

## Kontekst

pathon będzie publikowany na crates.io i używany wewnątrz Bevy (MIT OR Apache-2.0), Godota (MIT) i własnego silnika. Część kodu przychodzi z zewnątrz:
- porty shaderów (FSR 1 EASU / RCAS od AMD, MIT),
- zależności Rust (wgpu, wesl, sp-fidelity-wgpu: MIT / Apache-2.0),
- gdext w rozszerzeniu Godota (MPL-2.0),
- SDK producentów w paczkach vendorowych (FSR SDK: MIT; DLSS SDK: licencja NVIDIA, poza planem).

## Decyzja

- Cały kod pathon na licencji **`MIT OR Apache-2.0`**, w każdym crate'cie (`license = "MIT OR Apache-2.0"`), pliki `LICENSE-MIT` i `LICENSE-APACHE` w katalogu głównym.
- Przeniesiony kod obcy zachowuje oryginalny nagłówek licencji, a paczka ma plik `NOTICE` z jego źródłem.
- Binarki i nagłówki SDK producentów nie trafiają do repo. Paczka vendorowa znajduje SDK przy buildzie albo dostaje ścieżkę od użytkownika, a jej strona w docs podaje licencję SDK.
- `pathon-godot` linkuje gdext (MPL-2.0). MPL działa na poziomie pliku, więc nie zmienia licencji naszych plików.

## Rozważane opcje

| Opcja | Za | Przeciw |
| --- | --- | --- |
| **MIT OR Apache-2.0** | standard ekosystemu Rust; ta sama co Bevy i wgpu, więc zero tarcia przy integracji; Apache daje grant patentowy | brak ochrony przed zamknięciem forków |
| MIT | najprostsza | brak grantu patentowego; odstaje od Bevy |
| MPL-2.0 | zmiany w plikach pathon muszą wrócić do otwartego kodu | tarcie przy wbudowywaniu w silniki zamknięte; odstaje od Bevy i wgpu |

## Konsekwencje

- pathon można wbudować w zamknięty silnik i w projekty Bevy bez osobnej analizy licencji.
- Każda nowa zależność musi mieć licencję zgodną z dystrybucją na `MIT OR Apache-2.0`.

## Do zrobienia

1. [x] Potwierdzić licencję (autor, 2026-11-20).
2. [ ] `deps-check` (M0-02) odrzuca zależności spoza listy: MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0; MPL-2.0 tylko w `hosts/`.
