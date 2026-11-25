# pathon-hot

Hot-reload shaderów z dysku (M3-06, [ADR-0040](../adr/0040-hot-reload-shaderow.md)): obserwuje katalogi z plikami `.wgsl` i `.wesl`, podmienia zmienione źródła w `BackgroundCompiler` (`pathon-shader`) i pilnuje, żeby **błąd w shaderze nigdy nie wyłączył rysowania**. Narzędzie deweloperskie w `tools/` (nie w domknięciu rdzenia, nie w `crates/`); tylko natywnie (`notify` 8; na wasm crate jest pusty). Paczki o nim nie wiedzą: wołasz je z hosta, obok `BackgroundCompiler::pump`.

## Użycie

```rust,ignore
let config = HotConfig::new()
    .with_shader_dir("assets/shaders")      // każdy plik to shader główny: `fx/blur.wgsl` → klucz `fx/blur`
    .with_module_dir("assets/modules");     // każdy plik to moduł: `lighting/pbr.wgsl` → `package::lighting::pbr`
let mut reloader = HotReloader::new(&config)?;

// co klatkę, zaraz po compiler.pump():
let report = reloader.pump(&mut compiler);
for resource in report.retired { /* RecordCtx::release_later(resource) */ }
```

- **`HotReloader::new`** czyta katalogi raz; pierwszy `pump` oddaje to kompilatorowi, więc woła się go przed pierwszym `shader_module`.
- **`pump`** nie czyta plików i niczego nie składa (to robi wątek obserwatora). Przekazuje przyjęte źródła do `BackgroundCompiler::add_root_file` / `add_module_file` / `remove_*` (etykieta pliku w diagnostyce to ścieżka z konfiguracji), zwalnia z cache'u potoki wyrzuconych modułów (`evict_pipelines`), zabiera `take_failures` i `take_warnings` kompilatora do raportu i loguje wszystko przez `log` (`hot reload: `shaders/quad.wgsl` reloaded`).
- **`HotReport`**: `reloaded`, `removed` (etykiety plików), `diagnostics` (K0001–K0007, błędy S01xx z plikiem i linią, nieudane buildy w tle S0001/S0002, ostrzeżenia S0106), `retired` (do zniszczenia, patrz niżej).
- **`HotConfig`**: `debounce` (60 ms ciszy po ostatnim zdarzeniu), `limits: SourceLimits`, `settle_pumps` (2), `poll_interval` (250 ms, odpytywanie, gdy system nie daje powiadomień o plikach: K0007).

## Błędny shader

Zmiana jest najpierw sprawdzana na kopii (`LibrarySnapshot::check`: wesl + naga dla **każdej permutacji, którą kompilator już zbudował**). Nie przechodzi → nie jest stosowana: kompilator, moduły i potoki zostają, log ma błąd w stylu rustc z plikiem i linią, a po nim K0006. Poprawiony plik wchodzi sam. Pliki zapisane razem są sprawdzane razem (moduł i shader, który go potrzebuje); zepsuty plik nie blokuje drugiego (próby pojedynczo, do 32 plików). Ten sam błąd nie jest zgłaszany drugi raz.

Błędy dopiero przy tworzeniu potoku (S0002, S0110) pojawiają się po podmianie, więc stare moduły i potoki **nie wychodzą w `retired`**, dopóki jakikolwiek build w tle zawiódł od ostatniej zmiany, i czekają `settle_pumps` pumpów bez niczego w kolejce. Reguła dla paczki: pytaj o potok co klatkę, w której rysujesz, i zmieniaj potok, którym rysujesz, dopiero na `Lookup::Ready` (wzorzec w `examples/src/hot_quad.rs`; `MissPolicy::Placeholder` z poprzednim potokiem robi to samo).

## Unieważnianie (pathon-shader)

- `ShaderLibrary` wyrzuca permutacje tylko shaderów, których zmiana dotyczy: zmieniony shader główny — własne, zmieniony albo usunięty moduł — shadery, które go importują (bezpośrednio lub przez inne moduły; zależności z ścieżek `package::…` w źródłach, `package_paths`). Moduł z `super::` zwija się do „wszystko”.
- `PipelineCache::evict_modules(&[ShaderModuleId]) -> Vec<Resource>` i `BackgroundCompiler::evict_pipelines` (PAT-123) zapominają potoki zbudowane z wyrzuconych modułów; trwające buildy z nich są niszczone po zakończeniu.
- `LibrarySnapshot` (`ShaderLibrary::snapshot`, `BackgroundCompiler::snapshot`): kopia źródeł i zbudowanych kluczy do sprawdzania zmiany na innym wątku.

## Ścieżki i wejście

- Czytane są tylko zwykłe pliki z rozszerzeniem `.wgsl` / `.wesl` pod kanoniczną ścieżką katalogu; dowiązania do katalogów (i junctions) nie są śledzone, dowiązanie do pliku tylko gdy jego cel leży w tym samym katalogu (K0001). Pliki tymczasowe edytorów (`x.wgsl.tmp`, `x.wgsl~`) są ignorowane, zapis atomowy (nowy plik + zmiana nazwy) działa.
- Plik ≤ `max_file_bytes` (1 MiB, K0002), tylko UTF-8 (K0003), nazwa modułu to identyfikatory (K0004).
- **Stos wesl (PAT-122).** wesl i naga przechodzą drzewo rekurencyjnie, a przepełnienie stosu przerywa proces (`catch_unwind` go nie złapie). Pomiar na debugu, Windows, wątek 1 MiB: zagnieżdżone bloki padają od ok. 100 poziomów, łańcuch operatorów od 400, nawiasy od 1600, łańcuch importów od 800 modułów (czas składania łańcucha 400 modułów: 2,6 s, 1600 modułów: 130 s); na wątku 2 MiB dwukrotnie więcej. Dlatego **każdy plik jest mierzony, zanim zobaczy go wesl** (`SourceLimits`: `max_nesting_depth` 32, `max_operators_per_statement` 128, `max_import_depth` 32), a przekroczenie to K0005 i poprzednia wersja zostaje; typowy shader zagnieżdża do ok. 10 poziomów. Sprawdzanie zmiany biegnie na wątku z 64 MiB stosu jako drugą warstwą. Testy `tests/paths_and_limits.rs`: 200 000 nawiasów, 100 000 bloków, 100 000 operatorów i 400 modułów w łańcuchu importów nie ruszają procesu.

## Przykład i test

- `cargo run -p pathon-examples --bin hot_reload` ([przykłady](../examples.md)): trójkąt malowany kolorem z `examples/shaders/hot_reload/modules/palette.wgsl`, log wypisuje przeładowanie i nowy kolor; zepsuj shader, a kolor zostaje, a log ma plik i linię.
- `examples/tests/hot_reload.rs`: ten sam scenariusz (zapis zwykły, zapis atomowy, błędny shader, poprawka) na `rhi-null` oraz, z `--features gpu-tests`, na Vulkanie z odczytem pikseli; zmiana widoczna w kilkadziesiąt milisekund, limit testu 2 s. Uruchom GPU:

```powershell
cargo test -p pathon-examples --features gpu-tests --test hot_reload
```

Testy kodu (`tools/pathon-hot/tests/`): `reload.rs` (podmiana, atomowy zapis, moduł, błąd zostawia starą wersję, usunięcie, oddawanie zasobów po ustabilizowaniu) i `paths_and_limits.rs` (dowiązania, rozmiar, UTF-8, głębokość).

## Ograniczenia

- Każdy zapis katalogu to pełny odczyt wszystkich plików; dobre dla dziesiątek plików, niepraktyczne dla tysięcy.
- Sprawdzenie przed zmianą widzi tylko zbudowane permutacje; permutacja zamówiona później zawiedzie przy pierwszym użyciu, jak bez hot-reloadu.
- `pump` zabiera błędy i ostrzeżenia kompilatora (`take_failures`, `take_warnings`); host, który ich potrzebuje, czyta je z `HotReport::diagnostics`.
