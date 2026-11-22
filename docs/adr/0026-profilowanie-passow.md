# ADR-0026: Profilowanie passów (`pathon-profile`)

**Status:** Przyjęta
**Data:** 2026-11-22

## Kontekst

[ADR-0007](0007-logowanie.md) zostawia spany `tracing` i integrację z Tracy / puffin w `pathon-profile` (M2-12). Rdzeń nie może od niego zależeć (budżet zależności, [ADR-0005](0005-hosty-i-tryby-integracji.md)), a paczki nie powinny same wstawiać znaczników: pass nazywa graf (`PassDesc::name`), a GPU-passy (`BeginRenderPass`, `BeginComputePass`) nagrywają paczki w swoich domknięciach `record`.

RHI ma już timestampy ([ADR-0020](0020-kontrakty-rhi-types.md) i M1): `Features::TIMESTAMP_QUERY`, `create_query_set`, `PassTimestamps` w opisie passu, `resolve_queries`, `Limits`-owy `timestamp_period_ns`. Brakuje miejsca, które przydzieli indeksy zapytań passom paczek i odczyta wynik.

## Decyzja

### Obserwator klatki zamiast zależności

- Trait `FrameObserver` żyje w `pathon-graph` (tam, gdzie wykonują się passy), a `pathon-api::graph` i `pathon-core` go re-eksportują. `pathon-profile` implementuje go i zależy od `pathon-api`; `pathon-core` nie zna `pathon-profile`.
- Hooki, wszystkie z pustą implementacją domyślną: `begin_frame`, `begin_pass` / `end_pass` (CPU, wokół `record` passu grafu), `gpu_pass` (prośba o `PassTimestamps` dla jednego GPU-passu), `end_frame` (obserwator dokleja własne komendy do listy klatki), `frame_committed`, `frame_abandoned`, `device_reset`. Hooki, które potrzebują urządzenia, dostają `&dyn DeviceApi` w argumencie, więc obserwator nie trzyma go i nie musi nic robić przy wymianie urządzenia poza zapomnieniem swoich zasobów.
- Obserwator ustawia się na rendererze (`Renderer::set_frame_observer`). Bez obserwatora koszt to jedno sprawdzenie `Option` na pass i na klatkę.

### Timestampy nakładane przez graf

- Po nagraniu passu grafu executor przegląda jego komendy; każdy `BeginRenderPass` / `BeginComputePass` bez własnych `timestamps` dostaje je od obserwatora (`gpu_pass(device, nazwa_passu_grafu, etykieta_gpu_passu)`). Nazwa spanu to nazwa z grafu, więc paczki nic nie robią. Pass, który ma własne `timestamps`, zostaje nietknięty.
- Wymaga `CommandList::commands_mut` w `pathon-rhi-types` (zmiana tylko addytywna; długości listy nie da się zmienić).
- `rhi-null` zaczyna sprawdzać `PassTimestamps` (zbiór zapytań istnieje, `begin != end`, oba indeksy mieszczą się w zbiorze; kod `R0104`) i odrzuca `create_query_set` bez `TIMESTAMP_QUERY`, jak wgpu. Dzięki temu test na `rhi-null` łapie złe indeksy.

### Odczyt wyników

- Obserwator z `pathon-profile` ma pierścień slotów (zbiór zapytań, bufor `QUERY_RESOLVE`, bufor `MAP_READ`) o długości `opóźnienie + 1`. `end_frame` dokleja `resolve_queries` i kopię do bufora do odczytu; po `frame_committed` odczytuje slot sprzed `opóźnienie` klatek przez `DeviceApi::read_buffer` i przelicza ticki na nanosekundy przez `timestamp_period_ns`.
- `read_buffer` blokuje (wgpu: `poll(Wait)`), więc profilowanie GPU zmienia potokowanie CPU/GPU. To narzędzie pomiarowe, nie tryb produkcyjny; domyślne opóźnienie to 2 klatki.
- Tryb A (`record_frame`): host zgłasza listę sam, później; odczyt przed zgłoszeniem daje zera. Host musi zgłosić listę przed upływem opóźnienia (dokumentacja strony crate'u).
- Na wasm (brak blokującego odczytu) i bez `TIMESTAMP_QUERY` profil wyłącza część GPU i raz zgłasza diagnostykę **T0001**; spany CPU działają dalej. Gdy klatka ma więcej GPU-passów niż pojemność zbioru (domyślnie 128), nadmiarowe nie są mierzone, raz zgłaszane jest **T0002**.

### Ujścia

- Trait `ProfileSink` w `pathon-profile` (spany CPU i zebrane czasy GPU) z implementacjami za feature'ami, wszystkie domyślnie wyłączone:
  - `tracing` — span `pass` z polem `name` na CPU i zdarzenia `gpu_pass` z polami `name`, `label`, `duration_ns`; subskrybenta (np. `tracing-tracy`) wybiera aplikacja;
  - `tracy` — `tracy-client`: strefy CPU nazwane passami grafu i kontekst GPU z kalibracją, w którym strefy GPU dostają surowe ticki. Tylko natywnie (`cfg(not(target_arch = "wasm32"))`);
  - `puffin` — scope'y CPU nazwane passami.
- Bez feature'ów `pathon-profile` zależy tylko od `pathon-api` i `pathon-diag`; `tracing`, Tracy i puffin nie trafiają do domknięcia rdzenia.

### Kody diagnostyk

Nowa litera **T** (profil): `T0001` (czasy GPU niedostępne), `T0002` (za dużo passów GPU w klatce).

## Rozważane opcje

| Opcja | Za | Przeciw |
| --- | --- | --- |
| **Trait w `pathon-graph`, impl w `pathon-profile`** | core bez zależności, jeden punkt wykonania passów, nazwy z grafu | jeden trait więcej w publicznym API grafu |
| `tracing` w rdzeniu | zero nowych traitów | sprzeczne z ADR-0007 (budżet rdzenia); spany nie dają timestampów GPU |
| Paczki same ustawiają `timestamps` | brak nakładania komend | każda paczka musi znać profiler; nazwy rozjeżdżają się z grafem |
| Timestampy wewnątrz passów (`TIMESTAMP_QUERY_INSIDE_PASSES`) | pomiar podpassów | Apple GPU nie wspiera ([ADR-0009](0009-nagrywanie-przez-liste-komend.md), komentarz przy `PassTimestamps`) |

## Konsekwencje

- Każdy GPU-pass paczki jest mierzony pod nazwą passu grafu bez zmian w paczce; pass grafu z kilkoma GPU-passami daje kilka wpisów o tej samej nazwie i różnych etykietach.
- Zrzut z Tracy wymaga uruchomienia przykładu z `--features tracy` i narzędzia `tracy-capture` (albo GUI) w wersji zgodnej z protokołem `tracy-client`; nie jest częścią CI.
- Zmiana kolejności passów w grafie zmienia indeksy zapytań, nie nazwy.
