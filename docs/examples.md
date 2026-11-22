# Przykłady

Crate `examples/` gra rolę hosta, więc wolno mu używać wgpu i winit wprost (jak własny silnik).

| Przykład | Task | Tryb | Co pokazuje |
| --- | --- | --- | --- |
| `clear` | M0-10 | C | runner: okno na wątku głównym, renderer bez paczek na wątku renderu |
| `examples/web` | M0-06 | C | `clear` w przeglądarce (WebGPU), przez runner bez wątków ([ADR-0014](adr/0014-web-przez-winit-i-runner-bez-watkow.md)) |
| `external_window` | M0-11 | C | host ma okno i pętlę (surowy winit), woła `render_frame` na swoim wątku — bez `pathon-platform` |
| `multi_view` | M2-17 | C | host ma dwa okna (surowy winit) i cel offscreen 256×256, jeden renderer, trzy kamery; co 120 klatek odczyt piksela offscreen do logu |
| `vertical_slice` | M2-18 | C | sześcian unlit przez cały stos w dwóch oknach; `--devtools` udostępnia klatkę dla [devtools](devtools.md) |
| `mesh_pool` | M4-01 | — | trzy meshe z jednej puli mega-buforów rysowane offscreen (wynik jako tekst), potem usunięcie jednego i reuse jego zakresu ([pathon-mesh](crates/pathon-mesh.md)); bez rendera i okna, na samym `Device` |
| `profile` | M2-12 | C | wycinek pionowy offscreen pod okiem `pathon-profile`: tabela czasów GPU passów na stderr; `--features tracy` / `puffin` strumieniuje do profilera ([pathon-profile](crates/pathon-profile.md)) |
| `scene_extract` | M4-02 | — | host z własnym `Vec` obiektów kopiuje je do render worlda (`Scene::extract`): spawn, ruch części obiektów, despawn; log pokazuje listę zmian do wgrania, `previous_model`, mapowanie `MeshId` ↔ `MeshHandle` i odrzucony stary uchwyt ([pathon-scene](crates/pathon-scene.md)); bez GPU i okna |
| `texture_load` | M4-13 | — | tekstura sRGB przez `TextureLoader`: wybór formatu według `Caps` i mipmapy w compute; szachownica pokazuje uśrednianie w świetle liniowym (każdy mip 188, nie 128), wynik jako log ([pathon-texture](crates/pathon-texture.md)); bez rendera i okna, na samym `Device` |
| `material_sphere` | M4-03 | — | siatka 5 × 3 kul z materiałem standardowym (dielektryk, metal, lakier z clearcoatem; chropowatość rośnie w kolumnach), jedno światło kierunkowe i stały ambient zamiast IBL; wynik do pliku PNG (argument, domyślnie `material_sphere.png`), `cargo run -p pathon-examples --bin material_sphere` ([pathon-material](crates/pathon-material.md)); bez rendera i okna, na samym `Device` |
| `hdr_output` | M4-16 | C | raport, jakie przestrzenie barw oferuje powierzchnia okna i co wyświetla (`PATHON_COLOR_SPACE`, `PATHON_BACKEND`); bez tonemappingu, tylko możliwości ([ADR-0034](adr/0034-wyjscie-hdr.md)) |
| `host_foreign_device` | M2-19 | B | host tworzy urządzenie i swapchain; pathon przyjmuje urządzenie, renderuje do tekstury hosta, host kopiuje ją na ekran |

```sh
cargo run -p pathon-examples --bin vertical_slice
```

Przykłady okienkowe zatrzymują się same, gdy ustawisz `PATHON_EXIT_AFTER_FRAMES=N`: po N wyrenderowanych klatkach proces kończy się kodem 0, a log zawiera raport `Caps` (tak CI uruchamia `clear` bez człowieka). Dziś obsługują to `clear` i `hdr_output`.

```sh
PATHON_EXIT_AFTER_FRAMES=120 cargo run -p pathon-examples --bin clear
```

## Przykład webowy

Wymaga `wasm-pack` i celu `wasm32-unknown-unknown`. Bez WebGPU strona pokazuje komunikat zamiast obrazu.

```sh
examples/web/build.sh        # albo examples/web/build.ps1
cd examples/web && python -m http.server 8000
# otwórz http://127.0.0.1:8000/index.html
```

`examples/web/verify.cjs` sprawdza zbudowany przykład w Chromium (adapter, klatki, kolor canvasu, `Caps` kontra `navigator.gpu`); opis i kody wyjścia w [weryfikacji platform](weryfikacja-platform.md).

Wyjście (`examples/web/pkg/`) nie jest w repozytorium. CI buduje przykład w release i wypisuje rozmiar `.wasm` (surowy i po gzip) w podsumowaniu zadania `wasm`.

## Test szwu hosta

`examples/tests/host_seam.rs` (feature `gpu-tests`) sprawdza generyczność rdzenia na prawdziwym GPU:

- **tryb B** — scena wyrenderowana do tekstury hosta na jego urządzeniu jest identyczna piksel w piksel z trybem C (własne urządzenie, cel offscreen);
- **tryb A** — `record_frame` + `interop::encode_into` w enkoderze hosta: pathon rysuje na obrazie hosta i go nie czyści.

```sh
cargo test -p pathon-examples --features gpu-tests
```

## Test zdarzeń okna

`examples/tests/window_events.rs` (feature `window-tests`, bez harnessu testowego, bo macOS wymaga wątku głównego) otwiera prawdziwe okno przez `WinitPlatform`, zmienia jego rozmiar z kodu i czeka na `PlatformEvent::Resized`. Bez wyświetlacza (Linux bez `DISPLAY` i `WAYLAND_DISPLAY`) test się pomija, chyba że ustawiono `PATHON_REQUIRE_WINDOW=1`. `PATHON_EXPECT_SCALE` (z `WINIT_X11_SCALE_FACTOR` na X11) dodatkowo sprawdza skalę okna.

```sh
cargo test -p pathon-examples --features window-tests --test window_events
```
