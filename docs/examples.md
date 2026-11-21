# Przykłady

Crate `examples/` gra rolę hosta, więc wolno mu używać wgpu i winit wprost (jak własny silnik).

| Przykład | Task | Tryb | Co pokazuje |
| --- | --- | --- | --- |
| `clear` | M0-10 | C | runner: okno na wątku głównym, renderer bez paczek na wątku renderu |
| `examples/web` | M0-06 | C | `clear` w przeglądarce (WebGPU), przez runner bez wątków ([ADR-0014](adr/0014-web-przez-winit-i-runner-bez-watkow.md)) |
| `external_window` | M0-11 | C | host ma okno i pętlę (surowy winit), woła `render_frame` na swoim wątku — bez `pathon-platform` |
| `multi_view` | M2-17 | C | host ma dwa okna (surowy winit) i cel offscreen 256×256, jeden renderer, trzy kamery; co 120 klatek odczyt piksela offscreen do logu |
| `vertical_slice` | M2-18 | C | sześcian unlit przez cały stos w dwóch oknach; `--devtools` udostępnia klatkę dla [devtools](devtools.md) |
| `host_foreign_device` | M2-19 | B | host tworzy urządzenie i swapchain; pathon przyjmuje urządzenie, renderuje do tekstury hosta, host kopiuje ją na ekran |

```sh
cargo run -p pathon-examples --bin vertical_slice
```

## Przykład webowy

Wymaga `wasm-pack` i celu `wasm32-unknown-unknown`. Bez WebGPU strona pokazuje komunikat zamiast obrazu.

```sh
examples/web/build.sh        # albo examples/web/build.ps1
cd examples/web && python -m http.server 8000
# otwórz http://127.0.0.1:8000/index.html
```

Wyjście (`examples/web/pkg/`) nie jest w repozytorium. CI buduje przykład w release i wypisuje rozmiar `.wasm` (surowy i po gzip) w podsumowaniu zadania `wasm`.

## Test szwu hosta

`examples/tests/host_seam.rs` (feature `gpu-tests`) sprawdza generyczność rdzenia na prawdziwym GPU:

- **tryb B** — scena wyrenderowana do tekstury hosta na jego urządzeniu jest identyczna piksel w piksel z trybem C (własne urządzenie, cel offscreen);
- **tryb A** — `record_frame` + `interop::encode_into` w enkoderze hosta: pathon rysuje na obrazie hosta i go nie czyści.

```sh
cargo test -p pathon-examples --features gpu-tests
```
