# pathon-rhi-types

Wspólne typy wszystkich backendów RHI: formaty, opisy zasobów, `Caps`, model `Access`, lista komend i trait `DeviceApi`. Zależy tylko od `bitflags` i `raw-window-handle` (M0-07).

## Najważniejsze typy

- **Handle'y generacyjne** (`BufferId`, `TextureId`…) i arena `Pool` — backendy trzymają w niej obiekty GPU; handle zwolnionego obiektu nie trafi do nowego obiektu w tym samym slocie.
- **`Caps`** — źródło prawdy o możliwościach adaptera: bity `Features`, `Limits`, matryca formatów. `Profile` (Web / Desktop / High) to tylko nazwany zestaw bitów.
- **Tablice bindingów** — `BindGroupLayoutEntry::count: Option<NonZeroU32>` oznacza stałą tablicę (`binding_array<T, N>`); zasoby dają `BindingResource::TextureViewArray` / `SamplerArray`. Bind groupy są niemutowalne: zmiana elementu to nowa grupa. Reguły i limity (`max_binding_array_elements_per_shader_stage`, `max_binding_array_sampler_elements_per_shader_stage`) opisuje ADR-0017; naruszenia to R0107. Shadery z tablicą wymagają `enable wgpu_binding_array;`.
- **Immediates i overrides** — `Features::IMMEDIATES` i `Limits::max_immediate_size` (profil Web ich nie ma). `RenderPipelineDesc::immediate_size` / `ComputePipelineDesc::immediate_size` (0 = brak) rezerwuje blok widoczny dla wszystkich etapów, a `set_immediates(offset, data)` na enkoderze passa zapisuje do niego dane (`Command::SetImmediates`, dane w `Box<[u8]>`). W WGSL jest to `var<immediate> nazwa: T;` bez dyrektywy `enable`. `ShaderStage::constants` to nazwane stałe `override` z WGSL jako `OverrideValue` (bity `f64`; opisy potoków zostają `Eq + Hash`), unikalne co do nazwy. Reguły opisuje ADR-0018; brak `IMMEDIATES` daje R0002, `immediate_size` ponad limit lub niewyrównany do 4 B albo zduplikowany override R0107, a `set_immediates` poza passem, bez potoku, niewyrównane do 4 B, poza `immediate_size` potoku albo draw/dispatch bez zapisanego całego bloku (`rhi-null`) R0104. Bez `IMMEDIATES` `max_immediate_size` wynosi 0.
- **Pipeline cache** — `DeviceRequest::pipeline_cache: Option<PipelineCacheData>` seeduje jeden cache na urządzenie, a `DeviceApi::pipeline_cache_data()` oddaje aktualny blob do zapisu (`None` bez cache). `PipelineCacheData` to newtype na bajtach: `as_bytes()` do zapisu, `unsafe from_bytes(Vec<u8>)` do wczytania (bajty muszą pochodzić z `pipeline_cache_data`, jak w wgpu). Bez `Features::PIPELINE_CACHE` blob jest ignorowany, a niepasujący blob nie jest błędem. Decyzje: ADR-0019.
- **`Access` + `SubresourceRange`** — stan zasobu po barierze. Dostęp shaderów dzieli się na `SAMPLED`, `STORAGE_READ` i `STORAGE_WRITE` (ADR-0016). Stan łączy dowolne odczyty albo dokładnie jeden rodzaj zapisu (wyjątek: `STORAGE_WRITE` z `STORAGE_READ`, `DEPTH_WRITE` z `DEPTH_READ`); `is_valid_state` dotyczy buforów, a `is_valid_texture_state` dodatkowo wymaga, by odczyty tekstury należały do jednej klasy układu (`SAMPLED` z `DEPTH_READ`, `STORAGE_READ` z `STORAGE_WRITE`, reszta osobno). `Access` ma 16 bitów (13 zajętych), żeby `Command` mieścił się w 32 B. Graf zapisuje plan barier jako `Command::TextureAccess { texture, range, from, to }`; backend wgpu je pomija, `pathon-rhi-null` je waliduje.
- **`CommandList`** — nagrywanie niezależne od backendu. `render_pass()` / `compute_pass()` zwracają enkoder, który zamyka pass przy `Drop`. Backend tłumaczy listę przy `submit` albo — w trybach A / B — do enkodera hosta (`interop`).
- **`ExternalCommand`** — callback paczki vendorowej z zadeklarowanymi dostępami; backend podaje mu swój natywny kontekst przez `&mut dyn Any`.
- **`DeviceApi`** — wszystkie metody biorą `&self`, bo urządzenie jest współdzielone między wątkiem głównym (tworzenie surface'ów) a wątkiem renderu (ADR-0004).
- **`MaybeSend` / `MaybeSendSync`** — `Send`/`Sync` natywnie, puste na wasm.

## Kontrakty

Decyzje i uzasadnienie: [ADR-0020](../adr/0020-kontrakty-rhi-types.md). Backend natywny ma polegać na tych regułach, a nie na tym, jak działa wgpu.

- **Bariera niesie stan poprzedni.** `texture_access(texture, range, from, to)`: `from` to stan, w którym jest każdy subresource z `range`; zakres z różnymi stanami dzieli się na kilka komend. `Access::empty()` jako `from` znaczy „zawartość nieważna” (UNDEFINED). `rhi-null` porównuje `from` ze śledzonym stanem każdego subresource'a (R0103); graf bierze go z `StateTracker`, z `initial` importów i ze stanu, w którym poprzednia klatka zostawiła teksturę z puli.
- **Zapis z hosta.** `write_texture` zachowuje dane, a zapisane subresource'y są w stanie `COPY_DST`: następna bariera zaczyna się od `from: Access::COPY_DST`, a `Access::empty()` jest błędem (R0103). `write_*` jest uporządkowane przed każdym `submit`, który zaczyna się po jego powrocie, z dowolnego wątku.
- **Układ texeli.** `write_texture` wymaga tylko, by `bytes_per_row` mieścił wiersz bloków; kopie bufor↔tekstura w liście komend wymagają `bytes_per_row` podzielnego przez 256. `rows_per_image` musi pokrywać wysokość, gdy kopiowany jest więcej niż jeden obraz (warstwa lub plaster 3D). Naruszenie to R0113.
- **Kolejność w passie.** `SetBindGroup` wolno przed `Set*Pipeline`; grupy zostają po zmianie potoku o zgodnym layoucie.
- **Wątki.** Natywnie każdą metodę `DeviceApi` wolno wołać z dowolnego wątku i współbieżnie z innymi (także `create_*_pipeline` w trakcie `submit`); wyjątek: surface'y tworzy wątek z oknem (ADR-0004). `destroy_*` zasobu użytego w zgłoszonej liście jest odroczone do końca pracy GPU. Na wasm wszystko działa na jednym wątku.
- **Współrzędne.** Konwencja WebGPU: NDC `x, y ∈ [-1, 1]` z `y` w górę, `z ∈ [0, 1]`; framebuffer i tekstury od lewego górnego rogu; przód trójkąta CCW. Dla backendu natywnego niezweryfikowane (M2-15).
- **Bez dynamicznych offsetów** w bind groupach; dane per draw idą przez immediates (ADR-0018) albo osobne grupy.
- **Rozrost struktur.** Do zamrożenia w M5-07 nowe pole w strukturze z publicznymi polami jest akceptowaną zmianą łamiącą; `#[non_exhaustive]` z konstruktorem dla struktur budowanych przez paczki i dla `Limits` / `Caps` / `AdapterInfo` dojdzie przed M5-07.

Kody błędów `RhiError::code()` mają strony w `docs/diag/`.
