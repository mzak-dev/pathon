# pathon-diag

Diagnostyka z kodem i podpowiedzią (M1-02). Każda decyzja runtime (wyłączenie paczki, fallback, konflikt ról, błąd grafu) kończy się `Diagnostic`.

- `Diagnostic { code, severity, message, hint }` istnieje w każdym buildzie. `Display` wygląda jak komunikat rustc: `warning[F0102]: …`, potem `= hint:` i `= docs:` ze ścieżką do strony kodu.
- `emit()` wypisuje przez `log` (ADR-0007) i tylko raz na proces dla tej samej diagnostyki, więc warunek sprawdzany co klatkę nie zalewa logu.
- Kody: **R** — RHI, **G** — graf klatki, **F** — paczki (render features), **P** — platforma, **M** — meshe i pula geometrii, **H** — wyjście HDR, **S** — shadery (kompilacja w tle). Rejestr to `CODES`; test sprawdza, że każdy kod ma stronę w `docs/diag/<KOD>.md`, a każdy kod `RhiError` jest zarejestrowany.
- Kody: **R** — RHI, **G** — graf klatki, **F** — paczki (render features), **P** — platforma, **M** — meshe, **S** — shadery (kompilacja w tle S0001–S0002, błędy źródła z lokalizacją S0101–S0111, cache potoków na dysku S0201–S0202). Rejestr to `CODES`; test sprawdza, że każdy kod ma stronę w `docs/diag/<KOD>.md`, a każdy kod `RhiError` jest zarejestrowany.

Renderer w stylu miette jest pominięty: tekst w formacie rustc wystarcza w dev i release. Błędy shaderów (M3-07) mają własny render z plikiem, linią i podkreśleniem w `pathon-shader` (`ShaderDiagnostic`, [ADR-0032](../adr/0032-bledy-shaderow-z-lokalizacja.md)) i do `Diagnostic` trafiają jako tekst, więc `Diagnostic` nie zna pozycji w źródle.
