# pathon

Fasada: re-eksport `Renderer` i typów z `pathon-core`, moduły `api` (kontrakt paczek), `rhi`, `diag`.

## Features

- **`runtime`** — runner dla trybu C we własnym silniku (ADR-0004); natywnie także `StdTaskPool` (ADR-0008). Działa też na `wasm32` (sekcja „Runner w przeglądarce”).
- **`winit`** (domyślny, włącza `runtime`) — `WinitPlatform` z `pathon-platform-winit`.

Bez features fasada to sam renderer — tak używają jej hosty (Bevy, Godot), które mają własną pętlę i wątki.

## Runner

```text
pathon::runtime::run(WinitPlatform::new(), Renderer::builder().with_feature(…), windows, |frame| views)
```

- Wątek główny: pętla platformy, tworzenie okien i surface'ów (wymóg macOS), budowa renderera i callback sceny co klatkę.
- Wątek `pathon-render`: `render_frame`, zmiany rozmiaru, odzyskanie urządzenia (`recover`).
- Kanał o pojemności 1: wątek główny czeka, gdy wątek renderu jest klatkę w tyle — opóźnienie wejścia to najwyżej jedna klatka.

Ograniczenie: po utracie urządzenia surface'y okien są odtwarzane na wątku renderu; na macOS trzeba to przenieść na wątek główny, gdy runner będzie testowany na tym systemie.

`StdTaskPool` dzieli zakres na tyle części, ile jest rdzeni, na `std::thread::scope`. Pułap: wątki startują przy każdym wywołaniu; stałe wątki robocze, gdy pomiar (M4-17) pokaże ten koszt.

## Runner w przeglądarce (ADR-0014)

Na `wasm32` `runtime::run` i `run_with_hook` mają te same sygnatury, ale bez wątków (`MaybeSend` w boundzie hooka nie wymaga tam `Send`).

- Okna powstają od razu, renderer budowany jest asynchronicznie (`RendererBuilder::build_async` w zadaniu `wasm-bindgen-futures`); rozmiar okien jest odczytywany ponownie przy dodawaniu ich do renderera, więc zmiany rozmiaru w trakcie startu nie przepadają. Do tego czasu klatki są pomijane.
- Potem każda klatka (`requestAnimationFrame`, przez platformę) wywołuje scenę, `render_frame` i hook na jedynym wątku. Utrata urządzenia: asynchroniczne `recover_async`, w tym czasie klatki są pomijane.
- Brak WebGPU (brak `navigator.gpu`, adaptera albo nieudane tworzenie urządzenia): runner loguje błąd i dodaje do `#pathon` (albo `<body>`) akapit `<p id="pathon-no-webgpu">` z komunikatem po polsku i angielsku, usuwając canvas. `run` zwraca `Ok`.
- Błędy po starcie (np. nieudane tworzenie renderera) trafiają do `log::error!`, bo `run` wraca, zanim przeglądarka skończy start.
- `StdTaskPool` jest tylko natywny.
