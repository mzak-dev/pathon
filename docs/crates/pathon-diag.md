# pathon-diag

Diagnostyka z kodem i podpowiedzią (M1-02). Każda decyzja runtime (wyłączenie paczki, fallback, konflikt ról, błąd grafu) kończy się `Diagnostic`.

- `Diagnostic { code, severity, message, hint }` istnieje w każdym buildzie. `Display` wygląda jak komunikat rustc: `warning[F0102]: …`, potem `= hint:` i `= docs:` ze ścieżką do strony kodu.
- `emit()` wypisuje przez `log` (ADR-0007) i tylko raz na proces dla tej samej diagnostyki, więc warunek sprawdzany co klatkę nie zalewa logu.
- Kody: **R** — RHI, **G** — graf klatki, **F** — paczki (render features), **P** — platforma, **S** — shadery (kompilacja w tle). Rejestr to `CODES`; test sprawdza, że każdy kod ma stronę w `docs/diag/<KOD>.md`, a każdy kod `RhiError` jest zarejestrowany.

Renderer w stylu miette jest pominięty: tekst w formacie rustc wystarcza w dev i release. Wraca, gdy będą potrzebne podświetlenia źródła (błędy shaderów, M3-07).
