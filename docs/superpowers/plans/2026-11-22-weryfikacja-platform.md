# Weryfikacja platform (M0-05, M0-06, M0-08, M0-09, M0-10, M1-01, M2-11) — plan

**Cel:** domknąć taski, które czekały na inny sprzęt albo przeglądarkę, dowodami, które da się powtórzyć: testami, jobami CI i skryptem przeglądarkowym. To, czego automat nie zrobi, zostaje na liście dla autora ([weryfikacja platform](../../weryfikacja-platform.md)).

## Kroki

1. Test `caps_report` w `pathon-rhi-wgpu` (`gpu-tests`): `Caps` kontra surowy wgpu na każdym natywnym backendzie; urządzenie przyjęte przez `adopt`; raport do `PATHON_CAPS_REPORT`.
2. `PlatformWindow::request_inner_size` + test `window_events` (bez harnessu, feature `window-tests`): `Resized` po zmianie rozmiaru z kodu; skala przez `WINIT_X11_SCALE_FACTOR`.
3. `clear` kończy się sam po `PATHON_EXIT_AFTER_FRAMES` klatek i loguje `Caps`.
4. `examples/web/verify.cjs`: przykład webowy w Chromium (adapter, klatki, piksel, `Caps` kontra `navigator.gpu`).
5. CI: `gpu` (raport Caps), `gpu-metal`, `window` (Linux/Xvfb, macOS, Windows), `window-wayland` (eksperymentalny), krok Chromium w `wasm`.
6. Plan: odhaczyć tylko to, co ma twardy dowód; reszta dostaje notę „Stan” z brakującymi punktami.

## Review Focus

1. Czy każdy odhaczony task ma dowód spełniający jego „Gotowe gdy” (M0-09, M2-11), a pozostałe nie są odhaczone bez przebiegu CI na Linuksie/macOS.
2. Poprawka widoku w `create_texture_view` zmienia zachowanie tylko tam, gdzie format opisu różni się od formatu tekstury wgpu (powierzchnie webowe); natywne tekstury, głębia i aspekty bez zmian.
3. Joby CI: Xvfb i lavapipe, headless weston i software WebGPU w Chromium nie mają gwarancji na runnerze — kroki, które mogą się nie uruchomić, są oznaczone (`continue-on-error`), a pozostałe kończą się błędem przy braku okna (`PATHON_REQUIRE_WINDOW`).
