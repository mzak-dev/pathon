# ADR-0004: Model wątków

**Status:** Przyjęta
**Data:** 2026-11-20

## Kontekst

pathon działa w czterech środowiskach o różnych regułach wątków:
- **macOS:** operacje na oknie i tworzenie surface'a (`CAMetalLayer`) muszą iść na wątku głównym.
- **wasm:** jeden wątek; typy wgpu nie są tam `Send`. Bevy włącza feature wgpu `fragile-send-sync-non-atomic-wasm`, a cargo unifikuje features, więc pathon musi się kompilować z nim i bez niego.
- **Bevy:** render world może działać na osobnym wątku (pipelined rendering), więc zasoby muszą być `Send`.
- **Godot:** callback `CompositorEffect` przychodzi na wątku renderu Godota.

Hosty (ADR-0005) mają własne pętle i wątki, więc core nie może narzucać swojego.

## Decyzja

**Core nie tworzy wątków.**
- `pathon-core` jest synchroniczny; wszystkie metody `Renderer` biorą `&mut self` i wykonują się na wątku wywołującego.
- `Renderer: Send` natywnie, więc host może go przenieść tam, gdzie renderuje.
- Równoległą pracę w klatce core zleca wstrzykniętemu `TaskPool` (ADR-0008); bez puli wszystko idzie szeregowo.

**Granice `Send` / `Sync`:**
- Natywnie `Device` z `pathon-rhi` jest `Send + Sync`.
- Na wasm nic nie wymaga `Send`. Ograniczenia zapisujemy przez własny marker `MaybeSend` / `MaybeSendSync` w `pathon-rhi-types`: na natywnych celach równy `Send` / `Send + Sync`, na wasm pusty.

**Surface tworzy wątek główny, używa go wątek renderu.** `Device::create_surface(SurfaceTarget)` jest wołane na wątku głównym (wymóg macOS); gotowy surface trafia do renderera przez `add_surface`. Konfiguracja, acquire i present idą tam, gdzie `render_frame`.

**Kto robi co, per tryb (ADR-0005):**

| Środowisko | Wątek, który woła pathon | Acquire / present | Resize |
| --- | --- | --- | --- |
| Własny silnik z runnerem `pathon` (feature `runtime`) | wątek renderu runnera | pathon, na wątku renderu | wątek główny wysyła nowy rozmiar, wątek renderu rekonfiguruje przed acquire |
| Własny silnik bez runnera | wątek, który wybierze aplikacja | pathon (tryb C) albo host (A / B) | aplikacja woła `resize` |
| wasm | jedyny wątek, z `requestAnimationFrame` | pathon | callback zmiany rozmiaru canvasu |
| Bevy | system Bevy (render world albo świat główny w trybie C) | tryb C: pathon; A / B: Bevy | tryb C: pathon czyta zmiany okien Bevy; A / B: Bevy |
| Godot | wątek renderu Godota (callback) | Godot | Godot |

**Runner w fasadzie.** Opcjonalny runner (`pathon`, feature `runtime`) dla trybu C we własnym silniku:
- Wątek główny obsługuje platformę i tworzy surface'y; wątek renderu ma `Renderer`.
- Między nimi kanał o pojemności 1 ze snapshotem sceny (podwójny bufor z M4-02), więc wątek renderu jest najwyżej jedną klatkę za głównym.
- Callbacki utraty urządzenia (M1-03) przychodzą na wątku, który woła `render_frame`.

## Rozważane opcje

| Opcja | Za | Przeciw |
| --- | --- | --- |
| **Core synchroniczny, wątki u hosta lub w runnerze** | działa w każdym hoście; jeden kod dla wasm i natywnie; testowalny bez wątków | runner to osobny kod do utrzymania |
| Core z własnym wątkiem renderu | prosta integracja w trybie C | konflikt z wątkiem renderu Bevy i Godota; na wasm niemożliwe; host traci kontrolę nad synchronizacją |

## Konsekwencje

- Test w M2-11 sprawdza, że core nie tworzy wątków.
- Opóźnienie wejścia w runnerze to najwyżej 1 klatka; więcej klatek w locie wymaga zmiany pojemności kanału i rewizji tego ADR.
