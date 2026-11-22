# pathon-platform-winit

`Platform` na winit 0.30 (M0-05) — jedyny crate, który importuje winit.

- `WinitPlatform::new().run(app)` uruchamia `EventLoop::run_app` na wątku głównym z `ControlFlow::Poll`.
- `resumed` → `SurfacesAvailable`, `suspended` → `SurfacesLost`; zdarzenia okien tłumaczone na `PlatformEvent`, `about_to_wait` → `App::on_frame`.
- Okna (`WinitWindow`) implementują `PlatformWindow` i `raw-window-handle`, więc trafiają wprost do `SurfaceTarget`.

Test M0-05 (`examples/tests/window_events.rs`, [opis](../examples.md)) zmienia rozmiar prawdziwego okna z kodu przez `PlatformWindow::request_inner_size` i czeka na `PlatformEvent::Resized`; CI robi to na Windows, macOS i Linuksie (X11 pod Xvfb, Wayland pod headless westonem). Zmiany skali (`ScaleChanged`) nie da się wymusić w CI, więc zostaje do ręcznego sprawdzenia ([weryfikacja platform](../weryfikacja-platform.md)).

## Web (ADR-0014)

Na `wasm32` ten sam crate obsługuje przeglądarkę; nie ma osobnego crate'u platformy.

- `Platform::run` używa `EventLoopExtWebSys::spawn_app` i wraca od razu — pętlą rządzi przeglądarka.
- `create_window` tworzy `<canvas>` w elemencie o id `pathon` (albo w `<body>`), z CSS `100% × 100%`. winit zgłasza `inner_size()` = 0×0 do pierwszego tyknięcia `ResizeObserver`, więc dopóki tak jest, `inner_size()` okna liczy rozmiar z elementu canvas (CSS × `devicePixelRatio`). Dalsze zmiany rozmiaru zgłasza `ResizeObserver` winita jako `Resized` w pikselach fizycznych.
- Klatka (`App::on_frame`) idzie z `RedrawRequested`, czyli z `requestAnimationFrame`; po każdej klatce platforma prosi o następną. `about_to_wait` jest tu pomijane.
- Ograniczenie: wiele okien na jednej stronie wywołałoby `on_frame` raz na okno; web ma jedno okno.
