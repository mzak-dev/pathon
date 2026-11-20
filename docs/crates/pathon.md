# pathon

Fasada: re-eksport `Renderer` i typów z `pathon-core`, moduły `api` (kontrakt paczek), `rhi`, `diag`.

## Features

- **`runtime`** — runner dla trybu C we własnym silniku (ADR-0004) i `StdTaskPool` (ADR-0008). Tylko natywnie.
- **`winit`** (domyślny, włącza `runtime`) — `WinitPlatform` z `pathon-platform-winit`.

Bez features fasada to sam renderer — tak używają jej hosty (Bevy, Godot), które mają własną pętlę i wątki.

## Runner

```rust,ignore
pathon::runtime::run(WinitPlatform::new(), Renderer::builder().with_feature(…), windows, |frame| views)
```

- Wątek główny: pętla platformy, tworzenie okien i surface'ów (wymóg macOS), budowa renderera i callback sceny co klatkę.
- Wątek `pathon-render`: `render_frame`, zmiany rozmiaru, odzyskanie urządzenia (`recover`).
- Kanał o pojemności 1: wątek główny czeka, gdy wątek renderu jest klatkę w tyle — opóźnienie wejścia to najwyżej jedna klatka.

Ograniczenie: po utracie urządzenia surface'y okien są odtwarzane na wątku renderu; na macOS trzeba to przenieść na wątek główny, gdy runner będzie testowany na tym systemie.

`StdTaskPool` dzieli zakres na tyle części, ile jest rdzeni, na `std::thread::scope`. Pułap: wątki startują przy każdym wywołaniu; stałe wątki robocze, gdy pomiar (M4-17) pokaże ten koszt.
