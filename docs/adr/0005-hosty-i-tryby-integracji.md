# ADR-0005: Hosty i tryby integracji

**Status:** Przyjęta
**Data:** 2026-11-20

## Kontekst

pathon ma działać w trzech hostach: we własnym silniku, zamiast renderera Bevy i w Godocie. Hosty różnią się tym, kto ma urządzenie GPU, kto steruje klatką i kto pokazuje obraz:
- Bevy 0.20.1 jest na wgpu 30 (jak pathon) i przyjmuje urządzenie z zewnątrz przez `RenderCreation::Manual`.
- Godot daje dostęp do swojego `RenderingDevice`, ale nie pozwala podmienić renderera bez forka silnika.

## Decyzja

**Core nie wie, kto jest hostem.** `pathon-core` nie ma pętli, okna, wątku (ADR-0004), ECS ani systemu assetów. Nowy host nie może wymagać zmian w `pathon-core`; jeśli wymaga, poprawiamy API.

**Trzy możliwości core'a** wystarczają do wszystkich hostów:

| Możliwość | Warianty |
| --- | --- |
| Urządzenie | pathon tworzy (`build()`) albo przyjmuje cudze (`with_device`, przez `interop::adopt`) |
| Klatka | `render_frame()`: pathon zgłasza na kolejkę; albo `record_into(enkoder)`: nagrywa do enkodera hosta w punkcie jego klatki |
| Cele | surface, offscreen albo tekstura hosta (`import_texture`) z deklarowanym `Access` na wejściu i wyjściu |

**Trzy tryby integracji**, wspólne nazwy dla wszystkich hostów:

| | A: paczki w rendererze hosta | B: scena do tekstury hosta | C: jedyny renderer |
| --- | --- | --- | --- |
| Urządzenie | host | wspólne (w Bevy tworzy je pathon i oddaje hostowi) | pathon |
| Zgłoszenie komend | host (`record_into`) | pathon na wspólnej kolejce (`render_frame`) albo host (`record_into`) | pathon |
| Acquire / present | host | host | pathon |
| Utrata urządzenia | host, potem `reset(new_device)` | właściciel urządzenia | pathon |
| Czas i numer klatki | host | host | host (aplikacja albo runner) |
| Własny silnik | tak | tak | tak |
| Bevy | opcjonalnie (M6-05) | M6-04 | M6-03 |
| Godot | `CompositorEffect` (M7-04) | `Texture2DRD` (M7-05) | niemożliwy |

**Adaptery hostów** to crate'y w `hosts/` z `kind = "host"`. Mogą używać `interop` backendu (jak paczki vendorowe) i robią całą konwersję konwencji hosta (osie, jednostki światła, wektory ruchu, materiały) na kontrakt slotów z M2-10. Paczki nie wiedzą, w jakim hoście działają.

**Wersje.** Tryby A i B na wgpu wymagają tej samej głównej wersji wgpu u hosta i w pathon. pathon podąża za wersją wgpu wspieranej wersji Bevy (szczegóły w ADR z M6-00).

**Dowód generyczności:**
- Przykład `host-foreign-device` (M2-19) i test generyczności (M4-20) sprawdzają tryby A i B na urządzeniu utworzonym poza pathon, zanim powstanie pierwszy adapter.
- Każdy adapter dopina się do testu z M4-20.

## Rozważane opcje

| Opcja | Za | Przeciw |
| --- | --- | --- |
| **Trzy możliwości w kreatorze `Renderer` + adapter per host** | core nie zna hostów; adapter to cienka warstwa extract + konwersji | adapter musi znać szczegóły hosta (to jego rola) |
| Trait `Host` implementowany przez każdy host | jedno miejsce opisujące kontrakt | każdy host ma inny cykl klatki, więc trait byłby sumą wyjątków; jedna implementacja na crate niczego nie zyskuje |
| Osobny renderer per host | pełna swoboda w każdym hoście | trzy kopie grafu i paczek — przeciwieństwo celu projektu |

## Konsekwencje

- Własny silnik może zacząć od trybu C z `pathon-platform` i przejść na B, gdy zechce mieć własne urządzenie, bez zmian w paczkach.
- Tryb C w Bevy wyłącza UI, sprite'y, tekst i gizmo Bevy (żyją w `bevy_render`); kto ich potrzebuje, bierze tryb B.
- Backend natywny (M9) nie współdzieli urządzenia z Bevy, więc tryby A i B w Bevy zostają na backendzie wgpu.
