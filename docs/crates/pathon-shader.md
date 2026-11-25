# pathon-shader

Składanie shaderów WGSL z modułów (M3-01, [ADR-0021](../adr/0021-importy-shaderow-wesl.md)), permutacje i kesze (M3-03, [ADR-0023](../adr/0023-permutacje-shaderow.md)), kompilacja w tle (M3-04, [ADR-0024](../adr/0024-kompilacja-w-tle.md)), błędy z lokalizacją (M3-07, [ADR-0032](../adr/0032-bledy-shaderow-z-lokalizacja.md)) i cache potoków na dysku (M3-05, [ADR-0019](../adr/0019-pipeline-cache-i-zrodlo-shaderow.md)). Crate zależy od `pathon-rhi-types`, `pathon-api` (`TaskPool`, `Diagnostic`) i naga, nie od backendów RHI ani wgpu: bierze tekst, oddaje tekst, który trafia do `create_shader_module`, oraz layouty bind groupów z refleksji ([ADR-0006](../adr/0006-wejscie-shaderow.md)).

- **`ShaderComposer::add_module(path, source)`** — rejestruje moduł pod ścieżką `path` (identyfikatory rozdzielone `::`, np. `colors` albo `lighting::pbr`); shader importuje go jako `package::<path>`. Dodanie modułu pod tą samą ścieżką zastępuje poprzedni, a moduły zależne od niego widzą nową treść. Moduł jest parsowany dopiero przy składaniu.
- **`ShaderComposer::compose(source, file_path)`** — składa shader od zera z aktualnych modułów i zwraca samodzielne źródło WGSL. `file_path` służy tylko do komunikatów błędów.
- **`ShaderComposer::compose_with(source, file_path, &ShaderFeatures)`** — jak `compose`, z włączonymi flagami `@if` (`compose` to brak flag).
- **`ShaderComposer::add_module_file(path, source, file)`** — jak `add_module`, z etykietą pliku, którą pokazują diagnostyki (bez niej: `package::<path>`); `remove_module(path)` zapomina moduł.
- **`ShaderError`** (`#[non_exhaustive]`) — `InvalidModulePath` dla złej ścieżki, `InvalidFeatureName` dla flagi, która nie jest identyfikatorem WGSL, `UnknownRoot` dla nieznanego shadera głównego, `Rhi` dla błędu `create_shader_module`, `Compose` dla błędu składania wesl, `InvalidWgsl` dla WGSL odrzuconego przez naga, `Diagnostic` dla stałej potoku bez override. Trzy ostatnie niosą `ShaderDiagnostic` z kodem S01xx, plikiem, linią i podpowiedzią ([Błędy shaderów](#błędy-shaderów-m3-07)); `ShaderError::diagnostic()` go zwraca. Komunikaty nie mają kolorów ANSI, niezależnie od terminala i zmiennych środowiska.
- **`reflect_bind_groups(wgsl, entry_points)`** — parsuje i waliduje złożony WGSL w naga (`Capabilities::all()`) i zwraca `Vec<BindGroupLayoutDesc>` indeksowany numerem grupy (M3-02).

- **`reflect_buffer_layout(wgsl, group, binding)`** — układ struktury za bindingiem `uniform` albo `storage` według naga (M4-03, [ADR-0030](../adr/0030-material-standardowy-i-hooki-wgsl.md)): `BufferLayout { element_stride, struct_size, members: Vec<MemberLayout { name, offset, size }> }`. Dla `array<T>` `element_stride` to stride, a `members` to pola `T`; dla `array<vec4<f32>>` `members` jest puste. Binding, którego nie ma albo który nie jest buforem, to `UnsupportedBinding`. Służy do sprawdzania, że struktura CPU (`#[repr(C)]`) ma te same offsety co w shaderze.

Składnia to WESL: `import package::colors::{pack, unpack};`, a `import` stoi przed `enable`.

```rust
# use pathon_shader::ShaderComposer;
# fn main() -> Result<(), pathon_shader::ShaderError> {
let mut composer = ShaderComposer::new();
composer.add_module("colors", "fn red() -> u32 { return 255u; }")?;
let wgsl = composer.compose("import package::colors::red;\nfn f() -> u32 { return red(); }", "shader.wgsl")?;
# Ok(())
# }
```

## Permutacje i kesze

Permutacja to flagi WESL `@if(flaga)` (deklaracje, instrukcje i `@else`, także w modułach). Wartości liczbowe nie są flagami: idą przez overrides ([ADR-0018](../adr/0018-immediates-i-overrides.md)) i należą do klucza potoku.

- **`ShaderFeatures`** — posortowany zbiór włączonych flag (nazwa: identyfikator ASCII, który nie jest słowem zarezerwowanym WGSL/WESL, więc nie `_`, `if`, `true` ani `false`; inaczej `InvalidFeatureName`): `with(name)`, `set(name, enabled)`, `iter()`. Flaga wyłączona jest nieobecna, więc inna kolejność, duplikaty i jawne wyłączenie dają równe zbiory.
- **`ShaderKey { root, features }`** — nazwa shadera głównego i flagi.
- **`ShaderLibrary`** — `add_module(path, source)` (zwraca `Result<Vec<ShaderModuleId>>`), `add_root(name, source)` (zwraca `Vec<ShaderModuleId>`), `wgsl(&key)` (złożony tekst `Arc<str>`, składany raz na klucz i zachowany także do refleksji), `module(device, &key)`: składa (przez `wgsl`) i tworzy moduł raz na klucz. Nieudane składanie ani tworzenie nie trafia do kesza. Rejestracja nowej ścieżki modułu albo nowej nazwy shadera głównego, a także ponowna rejestracja tej samej treści, nie rusza kesza. Zastąpienie istniejącego modułu inną treścią usuwa z kesza moduły shaderów, które go importują (bezpośrednio lub przez inne moduły; zależności z ścieżek `package::…` w źródłach, a moduł z `super::` w źródłach zwija to do „wszystkie”), zastąpienie shadera głównego — jego własne; **oddaje ich id**: biblioteka nie ma urządzenia, więc wołający sam woła `device.destroy(Resource::ShaderModule(id))`. Potoki zbudowane z usuniętych modułów zostają w `PipelineCache`, dopóki nie poda się tych id do `PipelineCache::evict_modules` (`BackgroundCompiler::evict_pipelines`), co robi `pathon-hot` ([ADR-0040](../adr/0040-hot-reload-shaderow.md)). `snapshot()` daje `LibrarySnapshot` (kopia źródeł i zbudowanych kluczy; `check()` składa i waliduje je ze zmianą), `package_paths(source)` ścieżki `package::` ze źródła. Jedna biblioteka obsługuje jedno urządzenie.
- **`PipelineCache`** — `render(device, &desc)` i `compute(device, &desc)`: kluczem jest opis potoku po posortowaniu `constants` po nazwie; permutacja wchodzi do klucza przez `ShaderModuleId`. Etykieta (`label`) opisu należy do klucza: ten sam opis z inną etykietą to osobny potok. Nieudane tworzenie nie jest zapamiętywane. Jeden kesz obsługuje jedno urządzenie.

Paczka może zależeć od `pathon-shader` obok `pathon-api`; rdzeń nie zależy od tego crate'u.

## Kompilacja w tle

`ShaderLibrary::module` i `PipelineCache::render` budują na wątku wołającego: pierwsze użycie permutacji zatrzymuje klatkę na składaniu wesl i kompilacji sterownika. `BackgroundCompiler` robi to samo poza wątkiem klatki ([ADR-0024](../adr/0024-kompilacja-w-tle.md), M3-04). Należy do jednej paczki i jednego urządzenia; paczka tworzy go w `setup` z uchwytów `SetupCtx::device_handle()` i `task_pool_handle()`.

```rust,ignore
// setup
let mut compiler = BackgroundCompiler::new(ctx.device_handle(), ctx.task_pool_handle(), BackgroundConfig::new());
compiler.add_root("lit", LIT_WGSL);
// każda klatka
compiler.pump();
for failure in compiler.take_failures() { record_ctx.report(failure.diagnostic()); }
let module = compiler.shader_module(&key);            // Lookup<ShaderModuleId>
if let Lookup::Ready(module) = module {
    let pipeline = compiler.render_pipeline(&desc_for(module)); // Lookup<RenderPipelineId>
    if let Some(pipeline) = pipeline.resolve(MissPolicy::Skip) { /* draw */ }
}
```

- **`Lookup<T>`** — `Ready(T)`, `Pending`, `Failed`; `ready()` i `resolve(MissPolicy)`.
- **`MissPolicy<T>`** — `Skip` (draw pominięty) albo `Placeholder(T)` (zastępczy potok z tym samym layoutem bind groupów i vertex buffers, np. zbudowany synchronnie w `setup`); `Pending` i `Failed` traktuje tak samo. Politykę wybiera paczka: placeholder wymaga wiedzy o layoutach potoku docelowego, której rdzeń nie ma (uzasadnienie i rekomendacja w ADR-0024).
- **Zlecenia** — `shader_module(&ShaderKey)`, `render_pipeline(&desc)`, `compute_pipeline(&desc)`: pierwsze zapytanie kolejkuje build, kolejne zwracają `Pending`, aż wynik wpadnie do kesza. Klucze są deduplikowane (potok po normalizacji `constants`, jak w `PipelineCache`), flagi w innej kolejności to ten sam klucz. Potok wymaga gotowego modułu, więc paczka prosi najpierw o moduł, a gdy jest `Ready`, buduje z jego id opis potoku. Zapytanie, które trafia w kesz, nie alokuje; po `Ready` zapamiętaj id.
- **`pump()`** — raz na klatkę, nigdy nie czeka: zbiera gotowe wyniki, uruchamia zakolejkowane zadania przez `TaskPool::try_spawn` do limitu `max_in_flight` (domyślnie `thread_count() - 1`, minimum 1) i zwraca `PumpReport`. Pula bez wątków (`SerialTaskPool`, wasm) oddaje zadanie, więc pump wykonuje je inline, ale najwyżej `inline_jobs_per_pump` (domyślnie 1) na klatkę: budżet jest liczbą, nie czasem (`Instant` nie działa na `wasm32-unknown-unknown`). Na wasm daje to rozłożenie kosztu w czasie; zacięcie jednej klatki jest równe kosztowi jednego buildu.
- **Wątki** — zadanie niesie migawkę źródeł (`ShaderComposer` kopiowany przy zapisie), uchwyt urządzenia i nadawcę `mpsc`; stanu kompilatora nie dotyka, więc nie ma blokad ani wzajemnego czekania. Crate sam nie tworzy wątków (reguła `no-threads`): wszystko idzie przez `TaskPool`.
- **Błędy** — nieudane składanie, tworzenie albo panika w zadaniu (przechwycona) daje `BuildFailure { subject, error }` w `take_failures()` (raz na błąd) i klucz w stanie `Failed`, bez ponawiania co klatkę. `BuildFailure::diagnostic()` to `Diagnostic` z kodem [S0001](../diag/S0001.md) (moduł) albo [S0002](../diag/S0002.md) (potok). `retry_failed()` zwalnia klucze; `add_root` / `add_module` z inną treścią zwalnia nieudane moduły.
- **Zmiana źródła w trakcie** — wynik zadania, które zaczęło się na starszych źródłach (inna generacja biblioteki), jest niszczony, a klucz zamawiany od nowa.
- **Drop i zasoby** — drop kompilatora niczego nie joinuje: ustawia flagę anulowania, kolejka przepada, a zasób, który zadanie dokończy po dropie albo który leży nieodebrany w kanale, jest niszczony przez przesyłkę na uchwycie urządzenia. Zasoby odebrane i keszowane oddaje `release_all()` (do `RecordCtx::release_later`, ADR-0022). `finish_all(max_wait)` kończy wszystko do końca (rozgrzewka, testy).

Testy: `tests/background.rs` (urządzenie `rhi-null` z opóźnieniem kompilacji 40 ms na build i bramką sterowaną z testu) sprawdza deduplikację, politykę, błędy i panikę, limit równoległości, brak joinu przy dropie bez wycieków, unieważnienie przy zmianie źródła oraz p99 kosztu klatki dla 40 nowych permutacji (budżet 10 ms przy buildzie 40 ms; test kontrolny pokazuje, że ścieżka blokująca budżet łamie); bez wątków liczy buildy na klatkę, a czas tylko wypisuje. `tests/background_gpu.rs` (`--features gpu-tests`) kompiluje 16 permutacji compute na realnym wgpu na wątkach puli, sprawdza ten sam budżet i wyniki obliczeń.

## Cache na dysku (M3-05)

`PersistentPipelineCache` zapisuje blob cache potoków urządzenia (ADR-0019: `DeviceRequest::pipeline_cache`, `DeviceApi::pipeline_cache_data`) między uruchomieniami. Rdzeń nie zna systemu plików: miejsce wskazuje host przez `CacheStore`.

```rust,ignore
let cache = PersistentPipelineCache::new(Arc::new(FileCacheStore::new(app_cache_dir.join("pipelines.bin"))));
let seed = cache.load();                                   // przed urządzeniem; problemy to ostrzeżenia w seed.warnings()
let device = Device::build(&DeviceRequest { pipeline_cache: seed.data().cloned(), ..request })?;
if let Some(warning) = seed.adapter_mismatch(device.caps()) { /* log */ }
// co klatkę (tanie) albo z wątku pomocniczego:
if let Some(Err(error)) = cache.save_if_due(&device, Instant::now()) { log(error.diagnostic()) }
// przy zamknięciu:
cache.save(&device)?;
```

- **`CacheStore`** — `load() -> io::Result<Option<Vec<u8>>>` i `store(&[u8]) -> io::Result<()>` dla jednego bloba; `store` musi podmieniać atomowo. `FnCacheStore::new(load, store)` składa go z dwóch closure, `FileCacheStore::new(ścieżka)` (cecha `fs`, nie na wasm) tworzy katalogi, zapisuje do pliku tymczasowego obok i zmienia nazwę (crash w trakcie zapisu zostawia poprzedni plik). Ścieżkę podaje host (katalog cache aplikacji); jeden plik na adapter, jeśli host przełącza adaptery.
- **Plik** to nagłówek (magic, wersja formatu, wersja pathon, adapter, który go zapisał, długość i suma kontrolna FNV-1a payloadu) i blob backendu. `load` nigdy nie zwraca błędu: pusty, ucięty, uszkodzony, obcy albo zapisany przez inną wersję pathon plik, a także błąd odczytu, to **ostrzeżenie [S0201](../diag/S0201.md)** w `seed.warnings()` i pusty seed. Blob jest przy tym walidowany przez backend (adapter, sterownik, wersja wgpu: ADR-0019) i odrzucany bez błędu. `adapter_mismatch(caps)` wyjaśnia po utworzeniu urządzenia, że blob pochodzi z innego adaptera lub sterownika.
- **`save(device)`** zapisuje bieżący blob: `Saved`, `Unchanged` (taki sam jak w store), `Empty` (sterownik nic nie trzyma, np. software; plik zostaje) albo `Unsupported` (brak `Features::PIPELINE_CACHE`, backend null). Błąd zapisu to `CacheError::Io` z `diagnostic()` ([S0202](../diag/S0202.md), ostrzeżenie). `save_if_due(device, now)` zapisuje co `with_save_interval` (domyślnie 60 s); pierwsze wywołanie tylko uruchamia zegar, a `now` podaje host, więc moduł nie czyta zegara (`Instant::now` panikuje na `wasm32-unknown-unknown`).
- **Wątki:** `PersistentPipelineCache` jest `Send + Sync`; `save` robi I/O, więc host woła je z wątku pomocniczego (przez `TaskPool::try_spawn`), nie z wątku klatki.
- Test: `tests/disk_cache.rs` (format, uszkodzenia, ostrzeżenia, atomowość, okresowy zapis, plik) i `tests/disk_cache_gpu.rs` (`--features gpu-tests`): trzy starty w osobnych procesach, drugi start 3,5–5,7× szybszy od pierwszego; wynik i uwagi o cache sterownika w [docs/bench/pipeline-cache.md](../bench/pipeline-cache.md).

## Błędy shaderów (M3-07)

Błąd w `.wgsl` pokazuje plik, linię i podpowiedź w **oryginalnym** pliku, mimo że złożony tekst ma inne linie i przemianowane nazwy ([ADR-0032](../adr/0032-bledy-shaderow-z-lokalizacja.md)):

```text
error[S0107]: cannot find `intensty` in this scope
 --> shaders/unknown_name.wgsl:5:15
  |
5 |     let lit = intensty * 2.0;
  |               ^^^^^^^^
  = hint: did you mean `intensity`?
  = docs: docs/diag/S0107.md
```

- **`ShaderDiagnostic`** — `code`, `severity`, `message`, `location: Option<SourceLocation>`, `hint`, `notes`; `Display` w stylu rustc, `to_diagnostic()` daje `pathon_diag::Diagnostic` (lokalizacja i linia źródła w komunikacie) do `pathon_diag::emit`. `SourceLocation { module, file, line, column, length, line_text, exact }`; `exact = false` oznacza, że znaleziono tylko deklarację (linia to jej początek, bez podkreślenia).
- **Kody** ([S0101](../diag/S0101.md)…[S0111](../diag/S0111.md)): składnia, nieznany moduł, nieznana nazwa w imporcie, cykl deklaracji, `@if`, flaga bez czytelnika (ostrzeżenie), nieznana nazwa, walidacja, binding, stała potoku bez override, inne błędy składania. S0001 i S0002 zostają dla błędów bez lokalizacji (nieznany shader główny, błąd RHI, panika).
- **Skąd lokalizacja:** `ShaderLibrary::wgsl` (a przez nią `module` i `BackgroundCompiler`) po złożeniu parsuje i waliduje WGSL w naga, więc błąd typu, nazwy czy bindingu wychodzi z plikiem i linią modułu albo shadera głównego, nie z linią złożonego tekstu. Mapowanie opisuje ADR-0032: deklaracja po zdemanglowanej nazwie, a w niej token po tokenie. Bez dopasowania tokenów (np. usunięta gałąź `@if` z tymi samymi tokenami) diagnostyka wskazuje nazwę deklaracji.
- **Etykiety plików:** `ShaderLibrary::add_root_file(name, source, file)` i `add_module_file(path, source, file)` (te same na `BackgroundCompiler`) ustawiają plik pokazywany w diagnostyce; host przekazuje ścieżkę `.wgsl`. `add_root` i `add_module` używają nazwy shadera / `package::moduł`. `remove_module` i `remove_root` (plik usunięty) unieważniają kesz jak zastąpienie; używa ich `pathon-hot` ([strona](pathon-hot.md)).
- **`check_pipeline_constants(wgsl, &ShaderStage)`** — sprawdza nazwy `constants` względem overrides złożonego WGSL (nazwa albo `@id`); literówka daje S0110 z podpowiedzią. `BackgroundCompiler` woła to sam przed utworzeniem potoku.
- **`BuildFailure`** — `code()` i `diagnostic()` dają kod i lokalizację S01xx, gdy błąd je ma; `located()` zwraca `ShaderDiagnostic`.
- **Ostrzeżenia (S0106):** flaga włączona w `ShaderFeatures`, której nie czyta żadne `@if`, to cicha gałąź `@else`; `ShaderLibrary::take_warnings()` i `BackgroundCompiler::take_warnings()` oddają ostrzeżenie z najbliższą czytaną flagą.
- Błąd naga dotyczy pierwszego problemu; kolejne wychodzą po poprawce. Błąd RHI po przejściu walidacji (np. brak funkcji urządzenia) nie ma lokalizacji.

Testy: `tests/diagnostics.rs` z plikami w `tests/shaders/*.wgsl` (składnia, typ, nieznana nazwa, nieznany import i element, kolizja bindingu, błąd w module, cykl, `@if`, duplikat, usunięta gałąź `@if`, flaga, override, `BackgroundCompiler`) sprawdza plik, linię, kod i podpowiedź.

## Refleksja

```rust
# use pathon_shader::reflect_bind_groups;
# fn main() -> Result<(), pathon_shader::ShaderError> {
let wgsl = "@group(0) @binding(0) var<uniform> tint: vec4<f32>;
            @fragment fn fs() -> @location(0) vec4<f32> { return tint; }";
let layouts = reflect_bind_groups(wgsl, &["fs"])?;
assert_eq!(layouts[0].entries[0].binding, 0);
# Ok(())
# }
```

- **Zakres:** do layoutu wchodzi binding, którego statycznie używa co najmniej jeden z podanych entry pointów (naga `ModuleInfo`); `visibility` to suma etapów tych entry pointów. Nieużywane bindingi są pomijane, jak w automatycznym layoutcie wgpu, więc ten sam moduł z innym zestawem entry pointów daje inny layout.
- **Wynik:** `Vec` do najwyższej używanej grupy (numer grupy musi być mniejszy niż 32, wyższy to `UnsupportedBinding`; limit chroni przed ogromną alokacją i przepełnieniem) (grupy bez bindingów są puste), wpisy posortowane po `binding`, `label` to `"reflected"`. Bez użytych bindingów wynik jest pusty.
- **Mapowanie:** `uniform` na `UniformBuffer`; `storage, read` na `StorageBuffer { ReadOnly }`, `read_write` na `ReadWrite`; tekstura próbkowana `f32`/`i32`/`u32` na `Float`/`Sint`/`Uint` (`Float` jest filtrowalne, format bez filtrowania poprawia wołający; `f32` multisampled to `UnfilterableFloat`, bo wgpu odrzuca filtrowalne tekstury multisampled), głębia na `Depth`, `multisampled` z typu; tekstura storage na `StorageTexture` z formatem i dostępem; `sampler` na `Filtering` (sampler użyty z teksturą całkowitą albo niefiltrowalną wołający zmienia na `NonFiltering`), `sampler_comparison` na `Comparison`; wymiar z typu tekstury; `binding_array<T, N>` na `count = Some(N)` z typem z `T`. Zmienne `var<immediate>` nie są bindingami. Layout odzwierciedla shader, nie walidację RHI: zapisywalny storage buffer w etapie vertex naga przyjmuje, a odrzuca dopiero tworzenie layoutu.
- **Konflikt slotu:** ten sam `(group, binding)` zadeklarowany jako dwie różne zmienne w różnych entry pointach scala się, gdy typ i `count` są równe (suma `visibility`), a w przeciwnym razie to `BindingConflict`. Dwie zmienne w jednym slocie użyte przez jeden entry point odrzuca już walidacja naga (`InvalidWgsl`).
- **Błędy** (`ShaderError`, bez paniki): `InvalidWgsl` (parsowanie lub walidacja naga, komunikat bez kolorów), `UnknownEntryPoint`, `UnsupportedStage` (entry point inny niż vertex, fragment, compute), `UnsupportedBinding` (acceleration structure, tekstura zewnętrzna, tablica bez stałego rozmiaru, storage texture `atomic`, tablicowa tekstura 1D/3D, format storage bez odpowiednika w `Format`), `BindingConflict`. Binding nieużywany przez podane entry pointy nie jest sprawdzany pod kątem wsparcia.

## Co zostaje, co się zmienia

- **Shader główny zachowuje nazwy** entry pointów, overrides i globali (także `pass1`, `scale0`) oraz wszystkie deklaracje, także nieużywane overrides: `ShaderStage::constants` adresuje override nazwą ([ADR-0018](../adr/0018-immediates-i-overrides.md)).
- **Z modułów zostaje tylko to, do czego prowadzi shader główny:** nieużywane bindingi, overrides, funkcje i entry pointy modułu nie trafiają do wyniku (`strip` z `keep_main`).
- **Deklaracje modułów dostają prefiks ścieżki** (`package_colors_pack`); `_` w ścieżce i nazwie jest zamieniane na `_<liczba>` (`package_cam__1unused_tex`). Override z modułu adresuj przez `@id(n)`, który zostaje; identyfikatory `@id` są jedną przestrzenią dla shadera głównego i wszystkich modułów, duplikat odrzuca dopiero naga.
- `var<immediate>`, `enable f16;` i `enable wgpu_binding_array;` (`binding_array`) przechodzą bez zmian, także z modułów.
- Złożony WGSL nie ma komentarzy i jest przeformatowany z AST; błędy naga z `ShaderLibrary` / `BackgroundCompiler` są mapowane na oryginalne pliki (ADR-0032), ale błędy backendu RHI wskazują linie po złożeniu.
- Lista słów zarezerwowanych w wesl jest ostrzejsza niż w naga (np. funkcja `get` jest odrzucana).

Testy GPU (`--features gpu-tests`) składają cztery permutacje compute z `@if` i sprawdzają różne wyniki; poza tym składają compute z importowaną funkcją i overridem z shadera głównego, uruchamiają go i czytają bufor; drugi test tworzy bind group layout i potok compute z layoutu z refleksji i sprawdza wynik.

## Ograniczenia

- Samo `compose` / `compose_with` sprawdza tylko składnię i rozwiązywanie importów (wesl bez `eval`): błąd typów, wywołanie niezdefiniowanej funkcji czy nieznane `enable` wychodzą z naga przy `ShaderLibrary::wgsl` / `module` / `BackgroundCompiler`, z pozycją w oryginalnym pliku (M3-07). Nieużywany import nieznanego modułu jest przyjmowany po cichu.
- Błędy składni, importów i cykli wskazują plik, który je zawiera (moduł albo shader główny).
- Flaga, której nikt nie włączył, jest wyłączona, więc literówka w nazwie po cichu wybiera `@else` albo pomija deklarację; ostrzeżenie S0106 łapie włączoną flagę, której nic nie czyta, a test paczki musi sprawdzać permutacje, których używa.
- Źródło z BOM jest przyjmowane (BOM jest obcinany).
- **Głębokość:** patologicznie głębokie wyrażenia lub długie łańcuchy importów mogą przepełnić stos (wesl przechodzi AST rekurencją), a nie zwrócić `Err`; limity są niższe w buildzie debug. To limit parsera wesl: źródła shaderów to kod deweloperski, a stacker ani wątek nie wchodzą w grę (ADR-0004, wasm). Szczegóły w ADR-0021.
- **Koszt:** składanie rośnie mniej więcej kwadratowo z liczbą deklaracji (kilkaset deklaracji to dziesiąte części sekundy); wynik jest keszowany (`ShaderLibrary::wgsl`), a składanie nowej permutacji idzie w tle (`BackgroundCompiler`). Cache potoków (nie złożonego WGSL) idzie na dysk przez `PersistentPipelineCache`; złożony WGSL nie jest zapisywany.
