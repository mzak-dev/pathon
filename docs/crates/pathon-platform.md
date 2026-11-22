# pathon-platform

Kontrakt platformy (M0-04) bez żadnej biblioteki okienkowej — zależy tylko od `raw-window-handle`.

- **`Platform::run(app)`** — platforma ma pętlę zdarzeń (na macOS: wątek główny) i woła `App::on_event` / `App::on_frame`.
- **`PlatformCtx`** — ważny tylko w callbacku: tworzy i zamyka okna (`create_window` zwraca `Arc<dyn PlatformWindow>`, który da się podać do `SurfaceTarget`), kończy pętlę.
- **`PlatformEvent`** — `SurfacesAvailable` / `SurfacesLost` zamiast `Resumed` (tak modeluje to winit 0.31; między tymi zdarzeniami wolno mieć surface'y), `Resized`, `ScaleChanged`, `RedrawRequested`, `CloseRequested`, `Input`.
- `PlatformWindow::request_inner_size` to prośba, nie rozkaz: platforma zawsze odpowiada `Resized`, gdy rozmiar się zmieni, niezależnie od backendu (także gdy backend zastosował rozmiar od razu i sam nie wysłał zdarzenia).
- Rozmiary są typowane: `LogicalSize` (f64, przed skalą) przy tworzeniu okna, `PhysicalSize` (piksele) w zdarzeniach.
- Wejście jest surowe (`InputEvent`); mapowanie klawiszy na akcje jest poza zakresem.

Renderer nie zależy od tego crate'u: własny silnik może podać dowolne okno z `raw-window-handle` (przykład `external-window`).
