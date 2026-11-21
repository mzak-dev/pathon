# pathon-platform-winit

`Platform` na winit 0.30 (M0-05) — jedyny crate, który importuje winit.

- `WinitPlatform::new().run(app)` uruchamia `EventLoop::run_app` na wątku głównym z `ControlFlow::Poll`.
- `resumed` → `SurfacesAvailable`, `suspended` → `SurfacesLost`; zdarzenia okien tłumaczone na `PlatformEvent`, `about_to_wait` → `App::on_frame`.
- Okna (`WinitWindow`) implementują `PlatformWindow` i `raw-window-handle`, więc trafiają wprost do `SurfaceTarget`.

Ręczny test M0-05: przykład `clear` reaguje na resize i zmianę skali na Windows, Linux (X11 i Wayland) i macOS.

## Web (ADR-0014)

Na `wasm32` ten sam crate obsługuje przeglądarkę; nie ma osobnego crate'u platformy.

- `Platform::run` używa `EventLoopExtWebSys::spawn_app` i wraca od razu — pętlą rządzi przeglądarka.
- `create_window` tworzy `<canvas>` w elemencie o id `pathon` (albo w `<body>`), z CSS `100% × 100%`. Rozmiar bufora to rozmiar kontenera razy `devicePixelRatio` (gdy kontener ma zerowy rozmiar — rozmiar z `WindowDesc`), więc `inner_size()` jest poprawny od razu. Dalsze zmiany rozmiaru zgłasza `ResizeObserver` winita jako `Resized` w pikselach fizycznych.
- Klatka (`App::on_frame`) idzie z `RedrawRequested`, czyli z `requestAnimationFrame`; po każdej klatce platforma prosi o następną. `about_to_wait` jest tu pomijane.
- Ograniczenie: wiele okien na jednej stronie wywołałoby `on_frame` raz na okno; web ma jedno okno.
