# ADR-0019: Pipeline cache, źródło shaderów i pola klucza potoku

**Status:** Przyjęta
**Data:** 2026-11-21

## Kontekst

Spike ash (`docs/spike-ash.md`, pozycje P, S i K) zostawił trzy decyzje przed M3: `Features::PIPELINE_CACHE` istnieje, ale `DeviceApi` nie umie wczytać ani oddać blobu cache; `ShaderModuleDesc` przyjmuje tylko WGSL i nie wiadomo, czy ma przyjmować też SPIR-V; klucz permutacji w M3-03 potrzebuje pełnej listy pól opisu potoku. Bevy i Godot są odłożone (ADR-0015), więc argument „Godot chce SPIR-V” na razie nie działa.

## Decyzja

### Pipeline cache

- `DeviceRequest::pipeline_cache: Option<PipelineCacheData>` — blob dla całego urządzenia, jeden cache na urządzenie.
- `DeviceApi::pipeline_cache_data(&self) -> Option<PipelineCacheData>` — aktualny blob do zapisania przez hosta; `None`, gdy backend nie ma cache albo funkcja nie jest włączona.
- `PipelineCacheData` to newtype na bajtach: `as_bytes()` do zapisu, `unsafe fn from_bytes(Vec<u8>)` do wczytania. Kontrakt `unsafe` przenosi kontrakt wgpu (`Device::create_pipeline_cache`): bajty muszą pochodzić z `pipeline_cache_data` — sterownik nie musi bronić się przed dowolnymi danymi. Blob otrzymany z gettera da się przekazać dalej bez `unsafe`.
- Cache działa tylko z włączonym `Features::PIPELINE_CACHE` (dziś tylko Vulkan). Bez funkcji blob z `DeviceRequest` jest ignorowany, żeby host mógł żądać funkcji jako opcjonalnej i zawsze podawać blob z dysku.
- Blob niepasujący do adaptera, backendu, sterownika albo wersji wgpu nie jest błędem: urządzenie startuje z pustym cache. `fallback: true` w wgpu obejmuje tylko blob uszkodzony lub ze starszego sterownika albo wgpu; blob z innego adaptera lub backendu wgpu odrzuca błędem walidacji, więc backend tworzy cache w zakresie błędów i przy każdym błędzie tworzy go ponownie bez danych.
- Kontrakt `from_bytes` dotyczy bajtów z `pipeline_cache_data` tego samego backendu; gdy pojawi się drugi backend produkujący bloby (ash), blob dostanie własny znacznik backendu albo kontrakt zostanie zaostrzony.
- Backend wgpu tworzy cache przy każdym urządzeniu z włączoną funkcją (także cudzym, bez danych) i przekazuje go do każdego tworzonego potoku. `rhi-null` zawsze zwraca `None`.
- Gdzie i pod jakim kluczem host zapisuje blob — M3-05.

### Źródło shaderów: tylko WGSL

`create_shader_module` przyjmuje wyłącznie WGSL (ADR-0006). Natywny backend tłumaczy WGSL przez naga na SPIR-V i w tym kroku rozwiązuje overrides (ADR-0018) — spike ash to potwierdził. Powody:

- jedno źródło dla `pathon-shader` (importy, M3-01) i refleksji (M3-02), która czyta moduł naga; gotowy SPIR-V ominąłby oba;
- profil Web przyjmuje tylko WGSL;
- czas kompilacji sterownika pokrywa pipeline cache, a parsowanie naga jest tanie.

Wrócimy do tego, gdy pomiar startu (M3-05) pokaże, że parsowanie WGSL jest wąskim gardłem, albo gdy wróci backend Godota. Wtedy `ShaderModuleDesc` dostanie wariant źródła — przed M5-07 to tania zmiana.

### Pola klucza potoku

Klucz potoku w M3-03 to hash całego opisu (`RenderPipelineDesc`, `ComputePipelineDesc` mają `Eq + Hash`), więc każde nowe pole trafia do klucza samo. Lista pól, które dojdą razem z pierwszym użytkownikiem (najpóźniej przed M5-07):

| Pole | Gdzie | Kiedy |
|---|---|---|
| bias głębi: `constant: i32`, `slope_scale: f32`, `clamp: f32` | `DepthState` | M4-07 (cienie) |
| stencil: test i operacje dla przodu i tyłu, maski odczytu i zapisu; wartość odniesienia jako komenda w passie | `DepthState` + `Command` | pierwsza paczka ze stencilem |
| `polygon_mode` (`Fill` / `Line` / `Point`) | `PrimitiveState` | narzędzia debugowania (`POLYGON_MODE_*`) |
| `unclipped_depth` | `PrimitiveState` | cienie bez clippingu (`DEPTH_CLIP_CONTROL`) |
| `alpha_to_coverage` (razem z `sample_count` w stanie multisamplingu) | opis potoku | M4-19 |

Pola `f32` porównują i hashują bity (jak `OverrideValue`), żeby opis zachował `Eq + Hash`.

## Konsekwencje

- Nowe pole w `DeviceRequest` i nowa metoda w `DeviceApi` — zmiana łamiąca dla implementacji `DeviceApi` przed M5-07.
- Kod hosta wczytujący blob z dysku ma jeden blok `unsafe`; to świadome przeniesienie ryzyka, które wgpu też zostawia aplikacji.
- Bez SPIR-V w API nie da się podać shadera skompilowanego poza naga (np. z DXC); to nie jest dziś potrzebne.

## Uzupełnienie (M3-05, 22.11.2026): gdzie host zapisuje blob

Pierwotna decyzja zostawiła M3-05 pytanie „gdzie i pod jakim kluczem”. Rozstrzygnięcie, bez zmian w `rhi-types`:

- **Rdzeń nie zna systemu plików.** `pathon-shader` ma trait `CacheStore` (`load` i `store` jednego bloba; `store` podmienia atomowo), `FnCacheStore` z dwóch closure i `FileCacheStore` za cechą `fs` (nie na wasm: plik tymczasowy obok i `rename`). Ścieżkę podaje host.
- **`PersistentPipelineCache`** opakowuje blob w nagłówek (magic, wersja formatu, wersja pathon, adapter, długość i suma FNV-1a) i **jedyny `unsafe` w `pathon-shader`**, wywołanie `PipelineCacheData::from_bytes`: payload po przejściu nagłówka, wersji i sumy kontrolnej to bajty, które zwrócił `pipeline_cache_data`, a backend sprawdza jeszcze swój nagłówek (adapter, sterownik, wersja wgpu). Pusty, ucięty, uszkodzony albo obcy plik to ostrzeżenie S0201 i pusty cache, nigdy błąd.
- **Klucz** to ścieżka (jeden plik = jeden blob), nie nazwa adaptera: adapter jest znany dopiero po utworzeniu urządzenia, a blob trzeba podać w `DeviceRequest`. Zamiast tego plik zapamiętuje adapter, który go zapisał, a `CacheSeed::adapter_mismatch` wyjaśnia po utworzeniu urządzenia, dlaczego backend go zignorował; host z kilkoma adapterami podaje ścieżkę na adapter.
- **Zapis:** `save` przy zamknięciu, `save_if_due(device, now)` okresowo (zegar podaje host, bo `Instant::now` panikuje na wasm), pomijany dla pustego bloba (sterowniki software) i dla takiego samego jak w store. Zapis robi I/O, więc host woła go z wątku pomocniczego.
- **Pomiar** ([docs/bench/pipeline-cache.md](../bench/pipeline-cache.md)): z wyłączonym cache sterownika drugi start 8 potoków jest 3,5–5,7× szybszy od pierwszego i 3–7× szybszy od startu bez cache; z włączonym cache sterownika AMD (domyślnie) drugi start jest równie szybki bez naszego bloba, więc zysk w tej konfiguracji daje sterownik. Własny blob ma sens tam, gdzie cache sterownika nie działa.
