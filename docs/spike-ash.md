# Spike M2-15: backend ash i mapowanie na Godot `RenderingDevice`

Wyrzucany spike (21.11.2026). Cel: znaleźć w `pathon-rhi-types` to, co zmusiłoby do przebudowy API przy backendach Vulkan (M9) i Godot (M7). Kod prototypu nie wchodzi do repo; zostają wnioski poniżej.

## Co zbudowano

Samodzielny projekt Cargo poza repo (`ash 0.38`, `naga 30.0.2`, ścieżka do `pathon-rhi-types`), headless, Vulkan 1.3 z `synchronization2`, `dynamic_rendering`, semaforem timeline i deskryptorami indeksowanymi. Tłumaczy `CommandList` z `Command::TextureAccess`, `BeginRenderPass`, `SetBindGroup`, `Dispatch` i `Transfer::TextureToBuffer`; WGSL → SPIR-V przez naga.

Uruchomione na AMD Radeon RX 9070 XT (Vulkan 1.4.359) z warstwą walidacji Khronos; ostatni przebieg: 0 komunikatów walidacji.

| Scenariusz | Wynik |
|---|---|
| Tekstura `Rgba8Unorm` 64×64 z 4 mipami, `TextureAccess` per mip (mip 0 `COLOR_ATTACHMENT`, mip 1 `SHADER_WRITE`) → `VkImageMemoryBarrier2` | działa |
| Jedno `TextureAccess` na 2 mipy w różnych stanach | backend rozbija na 2 bariery; wymaga własnej mapy stanów per (mip, warstwa) |
| Clear mipa 0 (dynamic rendering, widok jednego mipa), compute zapisujący mip 1 do `StorageTexture`, kopia obu mipów do bufora i odczyt | piksele zgodne (`[255,128,0,255]`, `[0,255,0,255]`) |
| `SetBindGroup` przed `SetComputePipeline` | działa po odroczeniu bindowania do `Dispatch` |
| Timeline semaphore jako oczekiwanie na `submit` | działa |
| Bindless: 1024 sampled images, `PARTIALLY_BOUND` + `UPDATE_AFTER_BIND` + `VARIABLE_DESCRIPTOR_COUNT`, `binding_array` w WGSL, odczyt z indeksów 3 i 700 | działa, ale layoutu **nie da się wyrazić** przez `BindGroupLayoutDesc` (zbudowany surowym ash) |
| Zgodność formatów i limity (zapytania o właściwości) | `D24_UNORM_S8_UINT` i `X8_D24` nieobsługiwane na tym GPU; ETC2 i ASTC niedostępne; `maxBoundDescriptorSets=32`, `maxPushConstantsSize=256` |

Nie sprawdzono na GPU: trójkąt (potok graficzny), próbkowanie przez sampler, `present` i swapchain, resize, depth, MSAA resolve, wiele kolejek, aliasing pamięci, `write_texture` / `write_buffer`, QuerySet, formaty skompresowane. Backend jest pominiętym szkicem: bez alokatora, bez pooli per ramka, bez bezpieczeństwa wątkowego (`&mut self` zamiast `&self`).

## Proponowane zmiany w `pathon-rhi-types`

Priorytet **M3** = przed startem M3 (refleksja z naga buduje layouty, permutacje budują potoki, więc zmiany w opisach layoutów i potoków kosztują najmniej teraz). Zmiany są osobnymi taskami; ten dokument tylko je zgłasza.

| # | Problem | Dowód | Zmiana | Łamiąca | Kiedy |
|---|---|---|---|---|---|
| 1 | Tablice bindingów są niewyrażalne, choć istnieją `Features::TEXTURE_BINDING_ARRAY`, `PARTIALLY_BOUND_BINDING_ARRAY` i `Limits::max_binding_array_elements_per_shader_stage`. `BindGroupLayoutEntry` nie ma liczby elementów, `BindGroupEntry` trzyma jeden zasób | Prototyp, bindless. Vulkan wymaga liczby, flag bindingu (`PARTIALLY_BOUND`, `UPDATE_AFTER_BIND`, `VARIABLE_DESCRIPTOR_COUNT`) i flagi puli; walidacja wymusiła, by tablica ze zmienną liczbą była **ostatnim** bindingiem grupy. Godot: `RDUniform` przyjmuje wiele `add_id` na binding (tablica o stałym rozmiarze), brak publicznego partially bound / update-after-bind | `BindGroupLayoutEntry { count: Option<NonZeroU32>, flags }`, `BindingResource::TextureViewArray` (analogicznie bufory), reguła „tablica ostatnia” w refleksji M3-02 | tak (pole w literałach) | M3 |
| 2 | `Access::SHADER_READ` nie odróżnia odczytu próbkowanego od odczytu storage; Vulkan wymaga innego layoutu (`SHADER_READ_ONLY_OPTIMAL` vs `GENERAL`). Kombinacje czytań (`COPY_SRC \| SHADER_READ`) nie mają wspólnego layoutu tylko do odczytu | Prototyp: `texture_sync` musi zgadywać, a kombinacje wymuszają `GENERAL` | Rozdzielić na `SAMPLED_READ` i `STORAGE_READ` (`SHADER_READ` zostaje jako suma); graf wybiera wg deklaracji passa | semantycznie (dodane bity; zmiana znaczenia `SHADER_READ` w walidatorze) | M3 |
| 3 | `submit` zwraca `()`: brak numeru zgłoszenia do czekania na konkretną pracę, do odroczonego niszczenia zasobów i do synchronizacji z hostem | Prototyp: timeline semaphore jest gotowy, ale nie ma jak go wystawić. `destroy` na Vulkanie musi być odroczone do ukończenia zgłoszeń | `submit(...) -> Result<SubmissionIndex, RhiError>`, `DeviceApi::wait_submission(SubmissionIndex)`, `is_submission_done` | tak (typ zwracany) | M3 |
| 4 | `TextureDesc` nie niesie informacji potrzebnych przy tworzeniu obrazu: lista formatów widoków (Vulkan `MUTABLE_FORMAT` + format list, Godot `add_shareable_format`) i kompatybilność z cube / 2D-array (Vulkan `CUBE_COMPATIBLE`, Godot `TextureType` cube jest właściwością tekstury, nie widoku) | Odczyt kodu; Godot: `RDTextureFormat.texture_type`, `shareable_formats`; `TextureViewDesc::format`/`Cube` bez odpowiednika w opisie tekstury. Nie ćwiczone na GPU | `TextureDesc { view_formats: Vec<Format>, cube_compatible: bool }` (lub wariant `TextureDimension::Cube`) | tak (pola) | M3 |
| 5 | Opisy potoków nie mają push constantów ani stałych specjalizacji (`VkPipelineLayout` ranges, `VkSpecializationInfo`; Godot: `*_list_set_push_constant`, `specialization_constants`). M3-03 (permutacje) naturalnie z nich skorzysta | Prototyp: układ potoku bez zakresów push constantów (limit sprzętu 256 B) | `push_constant_bytes: u32` (+ stage) w `*PipelineDesc`, `Command::SetPushConstants { offset, data }`, `specialization: Vec<(u32, SpecValue)>` w `ShaderStage` | tak (pola), nowy wariant `Command` jest `non_exhaustive` | M3 |
| 6 | Niespisane kontrakty, które backend Vulkan musi znać: (a) po `write_texture` / `write_buffer` zasób ma zachowane dane, a `Access::empty()` (UNDEFINED) je odrzuca; (b) kto jest źródłem prawdy o stanie (graf z `initial` z ADR-0011 czy backend); (c) `SetBindGroup` dozwolone przed `Set*Pipeline`; (d) `bytes_per_row`: wgpu wymaga wielokrotności 256, Vulkan tylko wielokrotności rozmiaru teksela; (e) `write_*` są uporządkowane względem następnego `submit`; (f) `destroy` zasobu użytego w zgłoszonej liście jest odroczone | Prototyp: bindowanie odroczone do `Dispatch`; przejście z UNDEFINED kasuje zawartość (spec Vulkan), więc backend musi pamiętać stan po uploadzie | Opis w `docs/crates/pathon-rhi-types.md`; opcjonalnie flaga „odrzuć zawartość” zamiast domyślnego `empty()` (patrz 9) | nie | M3 (dokument) |
| 7 | `Access::SHADER_*` i `UNIFORM` nie mówią, w których etapach zasób jest użyty; backend synchronizuje wszystkie etapy shaderów | Prototyp: `texture_sync` używa sumy VS+FS+CS. Poprawne, ale zbyt zachowawcze | Opcjonalne `stages: ShaderStages` w `TextureAccess` / `BufferAccess` | nie (nowy wariant lub pole opcjonalne) | później (wydajność) |
| 8 | Brak wyboru kolejki: `CommandList` i `submit` nie mówią, czy lista idzie na kolejkę graficzną, compute czy transferu; brak synchronizacji między kolejkami i własności zasobów | Prototyp: ten GPU ma 3 rodziny kolejek (gfx+compute x8, compute x8, transfer x1). Godot: jedna główna kolejka, brak semaforów | `QueueKind` na liście, `Features::ASYNC_COMPUTE`, zależności przez `SubmissionIndex` z #3 | nie (domyślnie `Graphics`) | później (M8-04) |
| 9 | Brak jawnego odrzucenia zawartości i aliasingu pamięci: `create_texture` zawsze własna pamięć; M9 ma aliasing z puli grafu (ADR-0011) | Plan M8-04 / M9-01; Godot: `texture_set_discardable` | `Command::DiscardTexture { range }` lub flaga w `TextureAccess`; „grupa aliasingu” przy `create_texture` | nie | później (M9) |
| 10 | `Format::Depth24PlusStencil8` nie ma odpowiednika na tym GPU (AMD, `D24_UNORM_S8_UINT` i `X8_D24` nieobsługiwane); format potoku (`DepthState::format`) musi być zgodny z teksturą | Prototyp, zapytania o formaty. Godot ma konkretne formaty, bez „24plus” | Backend mapuje na `Depth32FloatStencil8`, a `texture_desc()` zwraca format faktyczny; `Caps::formats` raportuje rzeczywiste możliwości | nie | później (M9-01) |
| 11 | `Transfer::BufferToTexture` / `TextureToBuffer` nie mają odpowiednika w publicznym API Godota (tylko `texture_update`, `texture_get_data(_async)`, `texture_copy`, `buffer_copy`) | Dokumentacja `RenderingDevice` (Godot 4.7) | `Features::BUFFER_TEXTURE_COPY` (brak = emulacja compute lub zakaz użycia w paczkach) i przenośna ścieżka odczytu `DeviceApi::read_texture` dla golden images | nie | później (M7-01) |
| 12 | Powierzchnie i import tekstur: `create_surface` / `acquire_surface` / `present_surface` są w `DeviceApi`, a Godot nie tworzy powierzchni (`draw_list_begin_for_screen` należy do silnika). `import_texture` istnieje tylko w interopie wgpu, nie w trait | Mapowanie Godota; `interop.rs` w `pathon-rhi-wgpu`. M9-04 też wymaga importu surowego `VkImage` (layout, rodzina kolejki) | Metody powierzchni z domyślnym `Err(Unsupported)` albo osobny trait `SurfaceApi`; `DeviceApi::import_texture(NativeTexture)` z opisem stanu początkowego | tak (rozdzielenie trait) lub nie (domyślne impl) | później (M7-00) |
| 13 | Godot: `draw_list_draw` nie przyjmuje `first_vertex` / `first_instance` / `base_vertex` (offsety siedzą w `vertex_array` / `index_array` RID); brak `draw_list_set_viewport` (viewport = `region` w `draw_list_begin`); brak `StoreOp` per pass (tylko `is_discardable` tekstury); bufor indeksów ma format ustalany przy `index_buffer_create` | Dokumentacja Godota. Nie zweryfikowano, jak mieszać użycia bufora (VERTEX\|STORAGE\|INDIRECT) | `Features::DRAW_BASE_OFFSETS`, `DYNAMIC_VIEWPORT`; `BufferDesc::index_format: Option<IndexFormat>`; dokumentacja, że `Draw` z niezerowym startem może kosztować RID na draw | nie / pole opcjonalne | później (M7-01) |
| 14 | `QuerySet` + `PassTimestamps` + `ResolveQueries` nie mapują się na Godota (`capture_timestamp(name)` i odczyt po stronie CPU) | Dokumentacja Godota | W Godocie `TIMESTAMP_QUERY` wyłączone, a profil pochodzi z hosta; ewentualnie odczyt przez `DeviceApi::read_timestamps` | nie | później (M7-01) |
| 15 | `DeviceApi: Send + Sync` i `&self` (ADR-0004) kontra `RenderingDevice` Godota: globalne urządzenie jest używane z wątku renderu silnika, lokalne (`create_local_rendering_device`) ma własne `submit` / `sync` | Dokumentacja Godota; plan M7-02 (wątek `CompositorEffect`) | Backend Godota owija uchwyt z asercją wątku (`unsafe impl Send/Sync` z uzasadnieniem) albo trait rozluźnia wymóg; decyzja w ADR z M7-00 | nie | później (M7-00) |
| 16 | `Features` i `Limits` bez pozycji potrzebnych natywnie: `BUFFER_DEVICE_ADDRESS`, `ASYNC_COMPUTE`, rozmiar push constantów (256 B tutaj), limity deskryptorów per etap i per zestaw (Godot ma osobne limity per typ w uniform secie) | Prototyp (`maxPushConstantsSize`, `maxPerStageDescriptorUpdateAfterBind*`); `limit_get` Godota | Dołożyć bity i pola; mapowanie zgrubne w Godocie | nie | później |

Podsumowanie: 16 zmian, z czego **6 przed M3** (#1–#6; #6 to sam dokument), 10 później.

## Czego nie trzeba zmieniać

- `Command` z `TextureAccess` per podzasób: mapuje się 1:1 na `VkImageMemoryBarrier2` (z mapą stanów w backendzie), a w Godocie jest jedynie walidowane, bo `RenderingDevice` śledzi stany sam.
- `ExternalCommand` z `&mut dyn Any`: pasuje zarówno do `VkCommandBuffer`, jak i do uchwytu `RenderingDevice`.
- Opisy samplerów, formaty wierzchołków, blend i `Features` jako flagi: tłumaczą się wprost.
- Pula deskryptorów nie wymaga pól w `BindGroupDesc`: jej rozmiar jest sprawą backendu.
- Shadery jako WGSL (ADR-0006): działa przez naga → SPIR-V. Jedna uwaga: `binding_array` wymaga w WGSL `enable wgpu_binding_array;`, więc takie shadery nie są przenośne do WebGPU i muszą być bramkowane przez `TEXTURE_BINDING_ARRAY`.

## Mapowanie RHI v1 na Godot `RenderingDevice`

Źródło: dokumentacja klasy `RenderingDevice` i klas `RDUniform`, `RDTextureFormat`, `RDTextureView`, `RDFramebufferPass` (Godot 4.7, docs.godotengine.org/en/stable). Opisy metod przeczytano ogólnie; semantyki szczegółów (np. współbieżne użycie bufora jako vertex i storage) nie zweryfikowano w działającym Godocie.

| pathon RHI v1 | Godot `RenderingDevice` | Uwagi |
|---|---|---|
| `create_buffer` (`BufferUsage`) | `vertex_buffer_create`, `index_buffer_create`, `uniform_buffer_create`, `storage_buffer_create` (+ `BufferCreationBits`) | Rodzaj bufora wybierany przy tworzeniu; wielorakie użycie do sprawdzenia; format indeksów z góry (#13) |
| `write_buffer`, `read_buffer` | `buffer_update`, `buffer_get_data`, `buffer_get_data_async` | `MAP_READ` bez odpowiednika (odczyt przez wywołanie) |
| `create_texture` | `texture_create(RDTextureFormat, RDTextureView)` | `TextureUsage` → `TextureUsageBits`; cube jako `texture_type` (#4) |
| `create_texture_view` | `texture_create_shared_from_slice` / `texture_create_shared` | Zakres mipów i jedna warstwa lub typ plastra; brak widoku „stencil only” |
| `write_texture` | `texture_update(texture, layer, data)` | Dane całej warstwy; `TexelLayout` bez wpływu |
| `create_sampler` | `sampler_create(RDSamplerState)` | Zgodne |
| `create_shader_module` + `*PipelineDesc` | `shader_create_from_spirv(RDShaderSPIRV)` + `render_pipeline_create` / `compute_pipeline_create` | Shader Godota łączy etapy (vertex+fragment w jednym RID); moduły pathon trzeba scalić przy tworzeniu potoku |
| `BindGroupLayout` | brak osobnego obiektu | Layout wynika z refleksji shadera |
| `BindGroup` | `uniform_set_create(uniforms, shader, set)` | Wymaga shadera i numeru zestawu: backend materializuje zestaw leniwie przy pierwszym użyciu z potokiem albo tworzy shader-stub z layoutu (SPIR-V z naga) |
| `Command::TextureAccess` / `BufferAccess` | brak (automatyczne bariery grafu RD); `barrier`, `full_barrier` istnieją, ale zbędne | Plan `Access` tylko walidowany (jak w M7-01) |
| `BeginRenderPass` | `framebuffer_create` + `draw_list_begin(framebuffer, draw_flags, clear_colors, depth, stencil, region)` | `LoadOp::Clear` = `DRAW_CLEAR_COLOR_n`, `DontCare` = flaga ignorowania; `Load` domyślnie; `StoreOp` bez odpowiednika (#13); jedna lista naraz |
| `BeginComputePass` / `Dispatch` | `compute_list_begin`, `compute_list_bind_compute_pipeline`, `compute_list_bind_uniform_set`, `compute_list_dispatch(_indirect)`, `compute_list_add_barrier` | Zgodne |
| `SetBindGroup` | `*_list_bind_uniform_set(list, set, index)` | |
| `SetVertexBuffer`, `SetIndexBuffer`, `Draw*` | `vertex_array_create`, `index_array_create`, `draw_list_bind_*`, `draw_list_draw`, `draw_list_draw_indirect` | Zakresy i `base_vertex` przez tworzenie RID (#13) |
| `SetViewport`, `SetScissor` | `region` w `draw_list_begin`; `draw_list_enable_scissor` | Viewport dynamiczny niedostępny |
| `Transfer::BufferToBuffer`, `TextureToTexture` | `buffer_copy`, `texture_copy` | |
| `Transfer::BufferToTexture`, `TextureToBuffer` | brak | #11 |
| `QuerySet`, `PassTimestamps` | `capture_timestamp`, `get_captured_timestamp_*` | #14 |
| `PushDebugGroup` | `draw_command_begin_label` / `end_label` | |
| `Command::External` | wywołanie na uchwycie `RenderingDevice` | |
| Powierzchnie | `draw_list_begin_for_screen`, `screen_get_*` (tylko okno silnika) | W Godocie pathon renderuje do tekstur hosta (`CompositorEffect`, `Texture2DRD`), więc #12 |
| `Caps` | `limit_get`, `has_feature`, `texture_is_format_supported_for_usage`, `get_device_name` | Limity per typ w zestawie, nie per grupa |
| `submit`, `wait_idle` | globalne urządzenie: silnik zgłasza sam; lokalne: `submit()` + `sync()` | #3, #15 |
| Formaty | `DataFormat` obejmuje odpowiedniki formatów Vulkan, w tym BC, ETC2, ASTC | Bez abstrakcyjnego `Depth24Plus` (#10) |

Najpoważniejsze utrudnienia dla M7-01: #1 (tablice), #11 (kopie bufor-tekstura), #12 (powierzchnie i import tekstur hosta), #13 (offsety draw i viewport) oraz leniwe tworzenie `uniform_set`. Żadne nie blokuje backendu, jeśli trafią do `Features` przed implementacją.

## Czego spike nie pokrył

- Potok graficzny, próbkowanie z samplerem, depth, MSAA, swapchain, resize i `present` (headless).
- Wiele kolejek, async compute, własność zasobów między kolejkami, aliasing pamięci, alokator pamięci GPU.
- `write_texture` / `write_buffer`, QuerySet, formaty skompresowane, wiele warstw tablicy (tylko barierki zakresu warstw w kodzie, bez uruchomienia).
- Godot: wszystko papierowo, bez uruchomionego silnika; nie czytano źródeł Godota, tylko dokumentacji API.
- Wydajność (liczba barier, koszt mapy stanów), zachowanie na innych sterownikach niż AMD.
