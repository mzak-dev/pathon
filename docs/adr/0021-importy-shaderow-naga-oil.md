# ADR-0021: Importy shaderów przez naga_oil

**Status:** Przyjęta
**Data:** 2026-11-21

## Kontekst

M3-01 potrzebuje składania WGSL z modułów (`#import`), zanim źródło trafi do RHI (ADR-0006: RHI przyjmuje złożony WGSL). Plan wskazał dwa kandydaty: naga_oil (+14 crate'ów) i wesl (+~50). Bevy 0.20 przeszło na WESL, więc trzeba było sprawdzić, czy naga_oil żyje.

Stan 21.11.2026: naga_oil 0.23.0 (14.07.2026) zależy od `naga ^30` — tej samej wersji co wgpu 30; wydania idą za wydaniami naga. wesl 0.6.0 ma własny parser WGSL (`wgsl-parse`), więc refleksja (M3-02) i tak musiałaby parsować źródło naga drugi raz.

## Decyzja

- Nowy crate `pathon-shader` składa moduły przez `naga_oil::compose::Composer` i oddaje **źródło WGSL** zapisane z `naga::Module` przez `naga::back::wgsl`. Ten sam `naga::Module` posłuży refleksji w M3-02.
- Składnia importów to składnia naga_oil: moduł deklaruje `#define_import_path pathon::nazwa`, a shader używa `#import pathon::nazwa` (albo `#import pathon::nazwa::{funkcja}`). Definicje preprocesora (`#ifdef`) zostają na M3-03 (permutacje).
- `pathon-shader` nie zależy od RHI ani od wgpu; łączy go z RHI kod wołający (dziś testy, w M3-03 rejestr shaderów w rdzeniu udostępniony paczkom przez `pathon-api`).
- Błędy składania to `ShaderError` z komunikatem naga_oil (plik i linia w tekście); kody S z lokalizacją są w M3-07.
- naga_oil jest wpinany przez `PINNED_DEPENDENCIES` w xtask tylko w `pathon-shader`.

## Konsekwencje

- Źródło przechodzi przez naga dwa razy (składanie, potem backend) — koszt znika w keszu (M3-04, M3-05).
- WGSL zapisany przez naga różni się od wejścia (nazwy z prefiksem naga_oil, brak komentarzy); błędy backendu wskazują linie w źródle po złożeniu, nie w module paczki — M3-07 musi to mapować.
- Gdy naga_oil przestanie nadążać za naga, przejście na wesl dotyczy tylko `pathon-shader`.
