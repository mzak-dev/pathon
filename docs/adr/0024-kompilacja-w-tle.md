# ADR-0024: Kompilacja permutacji w tle

**Status:** Przyjęta
**Data:** 2026-11-22

## Kontekst

Nowa permutacja shadera (ADR-0023) to koszt, którego klatka nie może ponosić: składanie wesl rośnie mniej więcej kwadratowo z liczbą deklaracji (ADR-0021), a `create_render_pipeline` oddaje kompilację sterownikowi (dziesiątki milisekund na Vulkanie i D3D12). ADR-0020 zapewnia, że natywnie `create_*` wolno wołać z dowolnego wątku, także w trakcie `submit`. Core nie tworzy wątków (ADR-0004), a crate'y poza `pathon` nie mogą ich spawnować (xtask `no-threads`).

`TaskPool` z ADR-0008 to blokujące `parallel_for` po zakresie indeksów: pożycza dane klatki, ale wraca dopiero po końcu pracy, więc nie nadaje się do zadania, które ma żyć dłużej niż klatka. `SetupCtx` daje paczce tylko pożyczone `&dyn DeviceApi` i `&dyn TaskPool` na czas jednego wywołania; zadanie w tle potrzebuje uchwytów na własność.

## Decyzja

### Wykonanie: `TaskPool::try_spawn`

- `TaskPool` dostaje metodę z implementacją domyślną: `fn try_spawn(&self, job: BackgroundJob) -> Result<(), BackgroundJob>`, gdzie `BackgroundJob = Box<dyn FnOnce() + Send + 'static>` (na wasm bez `Send`, ADR-0004). Pula, która ma wolny wątek, uruchamia zadanie bez czekania i zwraca `Ok`; pula bez wątków (domyślna `SerialTaskPool`, wasm) oddaje zadanie w `Err`, a wołający decyduje, kiedy je wykona. Dodanie metody z implementacją domyślną nie łamie istniejących pul; `parallel_for` i `thread_count` bez zmian.
- `StdTaskPool` (fasada `pathon`, feature `runtime`) implementuje `try_spawn` jednym nazwanym wątkiem na zadanie. Pułap jak w ADR-0008: koszt startu wątku, zamiana na stałe wątki robocze po pomiarze M4-17; liczbę równoległych zadań ogranicza kompilator (niżej), więc wątków nie przybywa bez granic. Adaptery hostów (Bevy, Godot) zmapują `try_spawn` na własne zadania asynchroniczne, gdy powstaną.
- `SetupCtx` dostaje `device_handle() -> Arc<dyn DeviceApi>` i `task_pool_handle() -> Arc<dyn TaskPool>`; rdzeń i tak trzyma oba w `Arc`. Paczka przechowuje je na czas życia urządzenia (po resecie dostaje nowe w `setup`).

### Kompilator w tle (`pathon-shader`)

`BackgroundCompiler` należy do jednej paczki i żyje na wątku klatki; z wątków roboczych korzystają tylko zadania, które nie dotykają jego stanu (migawka źródeł, uchwyt urządzenia, nadawca kanału `std::sync::mpsc`).

- Trzy rodzaje zleceń: moduł (`ShaderKey`), potok renderowania i potok compute (opis po normalizacji, jak w `PipelineCache`). Paczka prosi o moduł, a gdy jest gotowy, buduje opis potoku z jego `ShaderModuleId` i prosi o potok; zlecenia są deduplikowane (jeden build na klucz).
- Zadanie modułu składa WGSL na migawce `ShaderComposer` (kopia przy zapisie, więc `add_module` w trakcie nie zmienia trwającego składania) i tworzy moduł. Złożony WGSL zostaje w keszu biblioteki (`Arc<str>`): koszt kompozycji płaci się raz na klucz, a paczka czyta ten sam tekst do refleksji (M3-02) bez ponownego składania.
- `pump()` raz na klatkę: zbiera gotowe wyniki (nieblokująco), uruchamia zakolejkowane zadania do limitu równoległości (domyślnie `thread_count() - 1`, minimum 1), nic poza tym. Nigdy nie czeka na wynik.
- **Pula bez wątków (wasm, `SerialTaskPool`):** `try_spawn` oddaje zadanie, więc `pump()` wykonuje je inline, ale najwyżej `inline_jobs_per_pump` (domyślnie 1) na klatkę. Budżet to liczba zadań, nie czas: `Instant` panikuje na `wasm32-unknown-unknown`, a liczba jest deterministyczna w testach. Koszt jednego zadania jest dolną granicą zacięcia; na wasm daje to rozłożenie w czasie, nie eliminację, i tu test tylko mierzy.

### Polityka braku potoku: wybiera paczka

Zapytanie zwraca `Lookup<T>` (`Ready(T)`, `Pending`, `Failed`), a paczka stosuje `MissPolicy`:

- **`Skip`** — draw jest pomijany, dopóki potok nie jest gotowy;
- **`Placeholder(T)`** — draw używa podanego zastępczego potoku (np. zbudowanego synchronnie w `setup`, z tym samym layoutem bind groupów i vertex buffers).

`Lookup::resolve(policy)` daje `Option<T>`. Domyślna rekomendacja: **`Skip`** dla efektów i obiektów, których chwilowy brak jest niewidoczny albo akceptowalny (cienie, dekale, post-process), `Placeholder` dla geometrii głównej, gdzie dziura w kadrze jest gorsza od przybliżonego wyglądu. Placeholder musi być zgodny layoutem z potokiem docelowym, bo inaczej `set_pipeline` i bind groupy zostaną odrzucone (R0107), a rdzeń nie może tego zgadnąć; stąd wybór oddany paczce, nie rdzeniowi. `Failed` daje tę samą politykę co `Pending` (placeholder albo pominięcie), a przyczyna trafia do diagnostyki, więc stały błąd nie zamraża kadru.

### Błędy, anulowanie, zasoby

- Nieudane składanie albo tworzenie (także panika w zadaniu, przechwycona `catch_unwind`) to wpis w `take_failures()`: `BuildFailure { subject, error }` z `diagnostic()` (kody **S0001** moduł, **S0002** potok). Wpis jest raz, klucz zostaje `Failed` bez ponawiania co klatkę, dopóki `retry_failed()` albo zmiana źródła (`add_module` / `add_root` z inną treścią) go nie zwolni.
- Zmiana źródła podnosi generację biblioteki: wynik zadania ze starej generacji jest niszczony, a klucz zamawiany od nowa. Generacja jest liczona per shader główny od M3-06, a potoki z unieważnionych modułów zwalnia `BackgroundCompiler::evict_pipelines` ([ADR-0040](0040-hot-reload-shaderow.md)).
- **Drop kompilatora nie czeka na zadania** (brak deadlocka, wątki są odłączone): ustawia flagę anulowania, kolejka zakolejkowanych zadań przepada, a każdy zasób, który zadanie dokończy po dropie albo który leży nieodebrany w kanale, niszczy `Drop` przesyłki (RAII) na uchwycie urządzenia. Zasoby gotowe i odebrane (kesze) oddaje `release_all()` do `RecordCtx::release_later` (ADR-0022), bo GPU mogło już ich użyć.
- Kolejność blokad: żadnych; wątki robocze trzymają tylko własne dane i nadawcę, więc nie ma cyklu oczekiwania.

## Rozważane opcje

| Opcja | Za | Przeciw |
| --- | --- | --- |
| **`try_spawn` w `TaskPool` + kompilator w `pathon-shader`** | działa na każdej puli hosta; wasm bez wątków degraduje się do inline; core bez wątków | nowa metoda w kontrakcie puli |
| `parallel_for` na kolejce w klatce | brak zmian w `TaskPool` | blokuje klatkę do końca pracy, czyli dokładnie to, czego unikamy |
| Własny wątek w `pathon-shader` | prosto | łamie ADR-0004 i regułę `no-threads`; druga pula obok puli hosta (ADR-0008) |
| Tylko placeholder albo tylko pominięcie | mniej API | placeholder wymaga zgodnego layoutu potoku (wiedza paczki), pominięcie bywa gorsze wizualnie |
| Budżet czasowy na wasm (`Instant`) | dokładniejszy | `Instant::now` panikuje na `wasm32-unknown-unknown`; niedeterministyczny test |

## Konsekwencje

- `pathon-shader` zależy od `pathon-api` (`TaskPool`, `Diagnostic`); `pathon-api` nie zależy od `pathon-shader`, rdzeń dalej nie ciągnie wesl.
- `SetupCtx::new` (ukryty w dokumentacji, wołany tylko przez rdzeń) przyjmuje uchwyty `Arc`; rdzeń przekazuje te, które już ma.
- Nowy prefiks kodów **S** (shadery): S0001, S0002.
- Test „p99 klatki w budżecie” (natywnie): zlecenia N nowych permutacji na urządzeniu z opóźnieniem kompilacji; budżet 10 ms kosztu `pump()` i zapytań na klatkę przy opóźnieniu 40 ms na zadanie (czterokrotny zapas, więc wersja blokująca oblewa test deterministycznie, a poprawna ma tysiąckrotny zapas). Na wasm test liczy zadania na `pump()` i tylko mierzy czas.
- Wątki robocze wołają `create_*` równolegle z `submit` na tym samym urządzeniu; natywnie dopuszcza to ADR-0020, a backend wgpu jest wielowątkowy.
