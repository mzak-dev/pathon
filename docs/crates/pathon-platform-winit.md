# pathon-platform-winit

`Platform` na winit 0.30 (M0-05) — jedyny crate, który importuje winit.

- `WinitPlatform::new().run(app)` uruchamia `EventLoop::run_app` na wątku głównym z `ControlFlow::Poll`.
- `resumed` → `SurfacesAvailable`, `suspended` → `SurfacesLost`; zdarzenia okien tłumaczone na `PlatformEvent`, `about_to_wait` → `App::on_frame`.
- Okna (`WinitWindow`) implementują `PlatformWindow` i `raw-window-handle`, więc trafiają wprost do `SurfaceTarget`.

Ręczny test M0-05: przykład `clear` reaguje na resize i zmianę skali na Windows, Linux (X11 i Wayland) i macOS.
