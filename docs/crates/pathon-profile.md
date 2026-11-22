# pathon-profile

Profilowanie passów grafu (M2-12, [ADR-0026](../adr/0026-profilowanie-passow.md)): spany CPU i czasy GPU nazwane tak jak passy w grafie, przekazywane do `tracing`, Tracy albo puffin. To narzędzie, nie paczka: rdzeń o nim nie wie, bo obserwuje je przez trait `FrameObserver` (`pathon_api::graph::FrameObserver`, zdefiniowany w `pathon-graph`). Bez ustawionego obserwatora rdzeń płaci jedno sprawdzenie `Option` na pass i na klatkę.

## Użycie

```rust,ignore
use std::sync::Arc;
use pathon_profile::{Profiler, TracySink}; // TracySink: feature `tracy`

let profiler = Profiler::builder()
    .with_sink(Arc::new(TracySink::new()))
    .build();
let mut renderer = Renderer::builder()
    .with_device_request(DeviceRequest {
        optional_features: Features::TIMESTAMP_QUERY,
        ..DeviceRequest::default()
    })
    .with_frame_observer(profiler.clone())   // albo Renderer::set_frame_observer(Some(..)) / None
    .build()?;
```

Przy własnym urządzeniu (tryby A i B) `Features::TIMESTAMP_QUERY` włącza host na swoim urządzeniu. Przed zniszczeniem urządzenia, które przeżywa profiler, wołaj `Profiler::release(device)`; `Profiler::flush(device)` odczytuje zaległe klatki od razu (zamknięcie, testy).

## Jak to działa

- **Spany CPU.** Executor grafu woła `begin_pass` / `end_pass` wokół nagrywania każdego passu, z nazwą z `PassDesc::name`. Wywołania są zawsze sparowane, także gdy nagrywanie passu się nie uda.
- **Czasy GPU.** Po nagraniu passu executor prosi obserwatora o `PassTimestamps` dla każdego `BeginRenderPass` / `BeginComputePass`, który nie ma własnych (`FrameObserver::gpu_pass(pass, label)`). Paczki nie robią nic: pass grafu z kilkoma GPU-passami daje kilka wpisów o tej samej nazwie i różnych etykietach. Timestampy są na granicach passów (`TIMESTAMP_QUERY`; Apple GPU nie zapisują ich w środku passu).
- **Odczyt.** `Profiler` ma pierścień slotów (zbiór zapytań, bufor `QUERY_RESOLVE`, bufor `MAP_READ`) długości `opóźnienie + 1`. `end_frame` dokleja na koniec listy klatki `resolve_queries` i kopię do bufora odczytu; wynik jest czytany po `opóźnieniu` kolejnych klatkach (domyślnie 2) przez `read_buffer` i przeliczany przez `Caps::timestamp_period_ns`. Domyślna pojemność to 128 GPU-passów na klatkę (`with_gpu_pass_capacity`).
- **Koszt i ograniczenia.** `read_buffer` blokuje do końca pracy GPU, więc profilowanie GPU zmienia potokowanie CPU/GPU — to narzędzie pomiarowe. W trybie A (`record_frame`) host zgłasza listę sam; odczyt przed zgłoszeniem daje zera, więc zgłoś listę przed upływem opóźnienia. Odrzucona klatka (`frame_abandoned`) nie jest raportowana. Po `Renderer::reset` obiekty profilera są tworzone od nowa.
- **Diagnostyki.** [T0001](../diag/T0001.md): brak `TIMESTAMP_QUERY` albo web (brak blokującego odczytu) — zostają spany CPU. [T0002](../diag/T0002.md): więcej GPU-passów w klatce niż pojemność.

## Ujścia (`ProfileSink`)

| Ujście | Feature | Co robi |
| --- | --- | --- |
| `CollectingSink` | — | zapamiętuje spany i czasy GPU (testy, narzędzia) |
| `TracingSink` | `tracing` | span `pass` z polem `name` (cel `pathon::pass`) i zdarzenie `gpu_pass` z `name`, `label`, `duration_ns`, `frame` (cel `pathon::gpu`); subskrybenta, np. `tracing-tracy`, wybiera aplikacja |
| `TracySink` | `tracy` | strefy CPU o nazwach passów, znaczniki klatek i kontekst GPU `pathon GPU` ze strefami z timestampów. Tylko natywnie; startuje klienta Tracy; nasłuchuje wyłącznie na `127.0.0.1:8086` |
| `PuffinSink` | `puffin` | scope'y CPU o nazwach passów i wątek `pathon GPU`; włącza scope'y puffin i kończy klatkę puffin po zatwierdzeniu klatki renderera. Tylko natywnie |

Wszystkie feature'y są domyślnie wyłączone; bez nich crate zależy tylko od `pathon-api` i `pathon-diag`. Strefy GPU w Tracy i puffin kalibruje się pierwszym odebranym timestampem, więc ich długości i kolejność są dokładne, a przesunięcie względem stref CPU jest stałe (kilka klatek).

## Zrzut z Tracy

Klient `tracy-client` 0.19 zawiera Tracy 0.14.1; narzędzia (GUI i `tracy-capture`) muszą być w wersji 0.14.x.

```sh
# terminal 1: nasłuch zrzutu
tracy-capture -o pathon.tracy -f -a 127.0.0.1
# terminal 2: przykład (domyślnie 600 klatek, ~10 s)
cargo run -p pathon-examples --bin profile --features tracy
```

W otwartym zrzucie (`Tracy.exe pathon.tracy`) wątek renderu pokazuje strefy `clear output` i `unlit mesh`, a kontekst `pathon GPU` — te same nazwy z czasami GPU.

## Przykład

`examples/src/bin/profile.rs`: wycinek pionowy w celu offscreen, co sekundę tabela czasów GPU passów na stderr; z `--features tracy` / `puffin` strumień do profilera.
