# pathon-shader

Składanie shaderów WGSL z modułów (M3-01, [ADR-0021](../adr/0021-importy-shaderow-wesl.md)), permutacje i kesze (M3-03, [ADR-0023](../adr/0023-permutacje-shaderow.md)). Crate zależy od `pathon-rhi-types` i naga, nie od backendów RHI ani wgpu: bierze tekst, oddaje tekst, który trafia do `create_shader_module`, oraz layouty bind groupów z refleksji ([ADR-0006](../adr/0006-wejscie-shaderow.md)).

- **`ShaderComposer::add_module(path, source)`** — rejestruje moduł pod ścieżką `path` (identyfikatory rozdzielone `::`, np. `colors` albo `lighting::pbr`); shader importuje go jako `package::<path>`. Dodanie modułu pod tą samą ścieżką zastępuje poprzedni, a moduły zależne od niego widzą nową treść. Moduł jest parsowany dopiero przy składaniu.
- **`ShaderComposer::compose(source, file_path)`** — składa shader od zera z aktualnych modułów i zwraca samodzielne źródło WGSL. `file_path` służy tylko do komunikatów błędów.
- **`ShaderComposer::compose_with(source, file_path, &ShaderFeatures)`** — jak `compose`, z włączonymi flagami `@if` (`compose` to brak flag).
- **`ShaderError`** (`#[non_exhaustive]`) — `InvalidModulePath` dla złej ścieżki, `InvalidFeatureName` dla flagi, która nie jest identyfikatorem WGSL, `UnknownRoot` dla nieznanego shadera głównego, `Rhi` dla błędu `create_shader_module`, `Compose` dla błędu składania (komunikat wesl bez kolorów ANSI, niezależnie od terminala i zmiennych środowiska, poprzedzony `file_path`; błędy rozwiązywania importów nie wskazują importującego pliku, więc nazwa pochodzi stąd).
- **`reflect_bind_groups(wgsl, entry_points)`** — parsuje i waliduje złożony WGSL w naga (`Capabilities::all()`) i zwraca `Vec<BindGroupLayoutDesc>` indeksowany numerem grupy (M3-02).

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
- **`ShaderLibrary`** — `add_module(path, source)` (zwraca `Result<Vec<ShaderModuleId>>`), `add_root(name, source)` (zwraca `Vec<ShaderModuleId>`), `module(device, &key)`: składa i tworzy moduł raz na klucz. Nieudane składanie ani tworzenie nie trafia do kesza. Rejestracja nowej ścieżki modułu albo nowej nazwy shadera głównego, a także ponowna rejestracja tej samej treści, nie rusza kesza. Zastąpienie istniejącego modułu lub shadera inną treścią usuwa z kesza wszystkie moduły (każdy shader główny mógł go importować) i **oddaje ich id**: biblioteka nie ma urządzenia, więc wołający sam woła `device.destroy(Resource::ShaderModule(id))`. Potoki w `PipelineCache` zbudowane z usuniętych modułów zostają w keszu do M3-06 (hot-reload i unieważnianie potoków). Jedna biblioteka obsługuje jedno urządzenie.
- **`PipelineCache`** — `render(device, &desc)` i `compute(device, &desc)`: kluczem jest opis potoku po posortowaniu `constants` po nazwie; permutacja wchodzi do klucza przez `ShaderModuleId`. Etykieta (`label`) opisu należy do klucza: ten sam opis z inną etykietą to osobny potok. Nieudane tworzenie nie jest zapamiętywane. Jeden kesz obsługuje jedno urządzenie.

Paczka może zależeć od `pathon-shader` obok `pathon-api`; rdzeń nie zależy od tego crate'u.

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
- Złożony WGSL nie ma komentarzy i jest przeformatowany z AST; błędy backendu wskazują linie po złożeniu (mapowanie to M3-07).
- Lista słów zarezerwowanych w wesl jest ostrzejsza niż w naga (np. funkcja `get` jest odrzucana).

Testy GPU (`--features gpu-tests`) składają cztery permutacje compute z `@if` i sprawdzają różne wyniki; poza tym składają compute z importowaną funkcją i overridem z shadera głównego, uruchamiają go i czytają bufor; drugi test tworzy bind group layout i potok compute z layoutu z refleksji i sprawdza wynik.

## Ograniczenia

- Składanie sprawdza tylko składnię i rozwiązywanie importów (wesl bez `eval`): błąd typów, wywołanie niezdefiniowanej funkcji czy nieznane `enable` wychodzą dopiero z naga, z pozycją w złożonym tekście. Nieużywany import nieznanego modułu jest przyjmowany po cichu.
- Błędy składni wskazują moduł (`package::lighting::pbr`), błędy rozwiązywania importów nie wskazują importującego pliku.
- Flaga, której nikt nie włączył, jest wyłączona, więc literówka w nazwie po cichu wybiera `@else` albo pomija deklarację; test paczki musi sprawdzać permutacje, których używa.
- Źródło z BOM jest przyjmowane (BOM jest obcinany).
- **Głębokość:** patologicznie głębokie wyrażenia lub długie łańcuchy importów mogą przepełnić stos (wesl przechodzi AST rekurencją), a nie zwrócić `Err`; limity są niższe w buildzie debug. To limit parsera wesl: źródła shaderów to kod deweloperski, a stacker ani wątek nie wchodzą w grę (ADR-0004, wasm). Szczegóły w ADR-0021.
- **Koszt:** składanie rośnie mniej więcej kwadratowo z liczbą deklaracji (kilkaset deklaracji to dziesiąte części sekundy); wynik trzeba keszować (M3-04, M3-05).
