# Weryfikacja na platformach

Taski M0 i M1, które zależą od sprzętu albo przeglądarki, mają trzy źródła dowodu: przebieg CI (Linux, macOS, Windows), skrypt przeglądarkowy i czynności ręczne, których automat nie zrobi. Ta strona mówi, co jest czym, i jak powtórzyć każdy dowód.

## Co sprawdza automat

| Dowód | Gdzie | Co potwierdza |
| --- | --- | --- |
| `crates/pathon-rhi-wgpu/tests/caps_report.rs` (`gpu-tests`) | job `gpu` (lavapipe), `gpu-metal`, lokalnie | `Caps` zgadza się z surowym wgpu (funkcje, limity, matryca formatów); urządzenie powstaje na każdym natywnym backendzie systemu (Windows: DX12 i Vulkan, Linux: Vulkan, macOS: Metal); urządzenie utworzone surowym wgpu i przyjęte przez `interop::adopt` raportuje te same `Caps` co urządzenie pathon z tym samym deskryptorem. Raport każdego backendu trafia do podsumowania joba (`PATHON_CAPS_REPORT`) |
| `examples/tests/window_events.rs` (`window-tests`) | job `window` (Xvfb, macOS, Windows), `window-wayland` | prawdziwe okno przez `WinitPlatform`: rozmiar zmieniany z kodu (`PlatformWindow::request_inner_size`) wraca jako `PlatformEvent::Resized`, a `inner_size()` zgadza się ze zdarzeniem; na X11 drugi przebieg z `WINIT_X11_SCALE_FACTOR=1.5` sprawdza, że skala trafia do `scale_factor()` i do przeliczenia rozmiaru logicznego na fizyczny |
| `clear` z `PATHON_EXIT_AFTER_FRAMES=120` | job `window` (Linux/Xvfb na lavapipe, macOS), `window-wayland` | okno z rendererem rysuje 120 klatek przez runner (wątek renderu) i kończy proces sam; log zawiera raport `Caps` |
| `examples/web/verify.cjs` | job `wasm` (krok Chromium), lokalnie | przykład webowy w Chromium: urządzenie powstaje asynchronicznie, pętla klatek rusza, piksel canvasu ma kolor czyszczenia zakodowany jak natywnie (89, 124, 160), `Caps` w konsoli zgadzają się z `navigator.gpu` (funkcje i limity) |
| test rdzenia i `xtask deps-check` | `check`, `lint` | rdzeń nie tworzy wątków; runner webowy kompiluje się na `wasm32` |

Skrypt przeglądarkowy lokalnie:

```sh
examples/web/build.sh
mkdir site && cp examples/web/index.html site/ && cp -r examples/web/pkg site/pkg
npm install playwright   # w osobnym katalogu; NODE_PATH wskazuje jego node_modules
node examples/web/verify.cjs site
```

Kody wyjścia: 0 — wszystko się zgadza, 1 — któraś kontrola nie przeszła, 2 — przeglądarka nie ma adaptera WebGPU (środowisko, nie błąd). Dowolna przeglądarka: `PATHON_CHROME_EXECUTABLE` albo `PATHON_CHROME_CHANNEL`, dodatkowe flagi w `PATHON_CHROME_ARGS`.

## Czego automat nie zrobi (czynności ręczne)

- **Zmiana skali (`ScaleChanged`)** — żaden runner CI nie przesuwa okna między monitorami o innym DPI. Na Windows, macOS i Linuksie (X11 i Wayland): uruchom `cargo run -p pathon-examples --bin clear`, przeciągnij okno na monitor o innej skali (albo zmień skalę w ustawieniach systemu) i sprawdź, że obraz nie jest rozciągnięty ani przycięty. `WINIT_X11_SCALE_FACTOR` tylko ustawia skalę przy starcie.
- **Resize przez użytkownika** na prawdziwym menedżerze okien (przeciąganie krawędzi): ten sam przykład.
- **Wayland na prawdziwym kompozytorze** — job `window-wayland` używa headless westona; to nie jest GNOME ani KDE.
- **Safari 26** — przeglądarka tylko na macOS 26 / iOS 26: `examples/web/build.sh`, serwer lokalny, otwórz `index.html`, sprawdź kolor `rgb(89, 124, 160)` (kolorem czyszczenia jest (0,1; 0,2; 0,35) liniowo) i brak błędów w konsoli; w Safari bez WebGPU ma być komunikat w stronie.
- **macOS na prawdziwym GPU** — runner GitHuba to maszyna wirtualna; urządzenie Metal tam powstaje, ale sterownik jest inny niż na Macu użytkownika.

## Rezerwacja nazwy `pathon` (M0-00)

Tylko autor może to zrobić; nic nie jest publikowane automatycznie. Sprawdzone 22.11.2026: `https://crates.io/api/v1/crates/pathon` zwraca 404, czyli nazwa jest wolna (podobnie `pathon-api`, `pathon-core`, `pathon-rhi-wgpu` i pozostałe crate'y workspace'u).

Fasady z workspace'u nie da się dziś opublikować: `cargo publish --dry-run -p pathon` kończy się błędem „dependency `pathon-api` does not specify a version” (zależności ścieżkowe do crate'ów, których nie ma na crates.io). Dlatego rezerwacja to osobny, pusty crate o tej samej nazwie, poza repozytorium:

1. `cargo new --lib pathon` w pustym katalogu poza repo; w `Cargo.toml`: `version = "0.0.1"`, `description`, `license = "MIT OR Apache-2.0"` (jak w workspace), `repository = "https://github.com/mzak-dev/pathon"`, `readme = "README.md"`; README mówi, że to rezerwacja nazwy dla silnika renderowania pathon.
2. `cargo login` (token z konta autora na crates.io; nie wklejamy go nigdzie w repo).
3. `cargo publish --dry-run`, potem `cargo publish`.
4. Opcjonalnie `cargo owner --add <login>` dla współwłaścicieli.
5. Po publikacji: odhaczyć punkt w ADR-0002 i M0-00 w planie. Prawdziwą fasadę `pathon` opublikujemy później, gdy jej zależności będą na crates.io (wersje w `[workspace.dependencies]`).
