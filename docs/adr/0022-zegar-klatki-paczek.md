# ADR-0022: Zegar klatki dla paczek

**Status:** Przyjęta
**Data:** 2026-11-22

## Kontekst

Paczka, która trzyma dane per klatka (pierścień uniformów, bufor tymczasowy), musi wiedzieć, kiedy zaczyna się nowa klatka, a paczka, która wymienia obiekt GPU (bufor po rozroście, pipeline po zmianie formatu), musi go oddać do opóźnionego zwolnienia. Do tej pory paczka dostawała od hosta `FrameInput::frame_index` (przez `ViewCtx::frame_index`), a `DeferredRelease` żył wyłącznie w rdzeniu i w `GpuArray::sync`, który dostaje `release` i `frame` od wołającego.

Skutek: host, który woła `render_frame` albo `record_frame` z domyślnym `FrameInput` (`frame_index == 0`), nigdy nie restartował `UniformRing` w `UnlitMeshPack`. Po około 256 zapisach pierścień zwracał R0108, `record` kończył się błędem, a paczka była wyłączana na stałe (F0105). Ten sam błąd dawało ponawianie porzuconej klatki z tym samym numerem. Przyczyna leży w kontrakcie: paczki nie mają zegara należącego do rdzenia ani drogi do opóźnionego zwolnienia, więc poprawność paczki zależy od tego, czy host dobrze numeruje klatki.

## Decyzja

### Zegar należy do rdzenia

- `FrameInput::frame_index` znika. `Renderer` liczy sam i wystawia tylko odczyt: `Renderer::frame_index()`.
- `ViewCtx::frame_index` zastępują dwa pola tylko do odczytu, oba `u64`, wspólne dla wszystkich widoków jednej próby nagrania:
  - `frame` — liczba **zatwierdzonych** klatek (patrz [Pojęcia](../../CONTEXT.md)). Tym zegarem chodzi `DeferredRelease`.
  - `record_epoch` — rośnie na początku **każdej** próby `render_frame` / `record_frame` (po `ensure_device`), także tej, która skończy się porzuconą klatką. Na tej zmianie paczka restartuje scratch per nagranie, np. `UniformRing::begin_frame`.
- Dwa liczniki, bo mają różne niezmienniki. `frame` nie może rosnąć przy porzuconej klatce: zwolnienie zależy od tego, ile klatek faktycznie dotarło do GPU. `record_epoch` musi rosnąć także wtedy: ponowione nagranie zaczyna od pustego pierścienia, a nie od miejsca, w którym stanął porzucony poprzednik.
- Zegar uruchamiany przez runner (`FrameContext::frame_index` w `pathon`) zostaje licznikiem runnera dla domknięć sceny; nie trafia już do `FrameInput`.

### Zatwierdzenie przesuwa zegar

`frame += 1` i `DeferredRelease::collect` przeniesione są z `end_frame` do `commit_frame_state`, czyli tam, gdzie ADR-0020 zatwierdza stany (zaraz po udanym `submit`; w trybie A przy powrocie `record_frame`). Dzięki temu błąd `present` po udanym `submit` nie zatrzymuje zegara: lista już się wykonała, a jej zasoby są w użyciu przez GPU.

### Opóźnione zwolnienie z `RecordCtx`

`RecordCtx::release_later(resource: impl Into<Resource>)` ustawia obiekt GPU w kolejce `DeferredRelease` rdzenia przy bieżącym `frame`. Metoda jest tylko na `RecordCtx` (`setup` i `prepare_view` nie wymieniają obiektów w trakcie klatki).

Rdzeń pożycza `DeferredRelease` runnerowi na czas `run_view`, a domknięcie `fill`, które trzyma `RecordCtx`, działa wewnątrz niego, więc `RecordCtx` nie może trzymać drugiego `&mut DeferredRelease`. `RecordCtx` pisze do lokalnego `Vec<Resource>`, a rdzeń przenosi go do `DeferredRelease` przy `self.frame` po powrocie z `run_view` — także gdy `run_view` zwraca `Err`, bo paczka obiekt już wymieniła i stary uchwyt nie ma innego właściciela.

### Tryb A

Bez nowego API. Host zgłasza listę z `record_frame` po jakimś czasie, a `frame` rośnie przy nagraniu, nie przy zgłoszeniu. Host musi więc ustawić `with_frames_in_flight` na co najmniej tyle, ile klatek nagrania mija między `record_frame` a zakończeniem jego `submit` na GPU; inaczej obiekt oddany przez `release_later` może zostać zniszczony, zanim lista go użyje.

### `GpuArray::sync`

Poprawka obok: `sync` nie porzuca już zakresu `dirty` przed udanym zapisem. Nieudany `write_buffer` zostawia zakres, więc następny `sync` ponawia wysyłkę, zamiast zostawić na GPU nieaktualne dane.

## Konsekwencje

- Zmiana łamiąca: `FrameInput` traci pole, `ViewCtx` zmienia pola, `RecordCtx::new` (ukryty w dokumentacji) dostaje kolejkę zwolnień. Hosty, które numerowały klatki tylko dla paczek, mogą przestać; hosty, które liczą klatki dla siebie, czytają `Renderer::frame_index()` albo mają własny licznik.
- Poprawność paczki nie zależy już od numeracji hosta ani od tego, czy host ponawia klatki.
- `UnlitMeshPack` restartuje pierścień na `record_epoch`.
- Wszystkie widoki jednej próby dzielą `record_epoch`, więc pierścień nie restartuje się między widokami i ich zapisy się nie nakładają.
- R0108 dalej oznacza przepełnienie pierścienia, ale jest też używany w innych sytuacjach; rozdzielenie kodu to osobny temat poza tą decyzją.

## Odrzucone

- **Licznik od hosta (stan dotychczasowy).** Poprawność paczki zależy od hosta, który nie wie, że ma to robić; domyślny `FrameInput` psuje paczkę po kilkuset zapisach.
- **Restart pierścienia w rdzeniu.** Rdzeń nie zna pierścieni paczek (nie zna żadnej techniki), więc musiałby je rejestrować; paczka, która ma więcej niż jeden pierścień albo własny scratch, i tak potrzebuje sygnału.
- **`notify_submitted` (host zgłasza, że lista poszła).** Dodatkowa metoda, którą host może pominąć, i inna ścieżka dla trybu A niż dla `render_frame`; zatwierdzenie w rdzeniu już istnieje i wystarcza.
- **Epoka per widok.** Pierścień restartowałby się między widokami jednej klatki i drugi widok nadpisałby uniformy pierwszego, zanim lista trafi do GPU; wspólna epoka na próbę tego unika.
