# Spike M2-15: backend ash i mapowanie na Godot `RenderingDevice`

Wyrzucany spike (21.11.2026), poprawiony po przeglądzie eksperckim i dokończony 22.11.2026 (trójkąt, tekstura z samplerem, bindless pod walidacją, swapchain z resize / present: sekcja „Drugie uruchomienie”). Cel: znaleźć w `pathon-rhi-types` to, co zmusiłoby do przebudowy API przy backendach Vulkan (M9) i Godot (M7). Kod prototypu nie wchodzi do repo; zostają wnioski poniżej. Zmiany to osobne taski (M2-20…M2-24, M4-06, M4-11 w planie).

## Co zbudowano (pierwsze uruchomienie, 21.11.2026)

Samodzielny projekt Cargo poza repo (`ash 0.38`, `naga 30.0.2`, ścieżka do `pathon-rhi-types`), headless, Vulkan 1.3 z `synchronization2`, `dynamic_rendering`, semaforem timeline i deskryptorami indeksowanymi. Tłumaczy `CommandList` z `Command::TextureAccess`, `BeginRenderPass`, `SetBindGroup`, `Dispatch` i `Transfer::TextureToBuffer`; WGSL → SPIR-V przez naga. `Command::External` wpadło w gałąź „niemapowane”.

Uruchomione na AMD Radeon RX 9070 XT (Vulkan 1.4.359) z warstwą walidacji Khronos. **Ograniczenia dowodu:**

- Nie było debug messengera; `run.log` zawiera tylko stdout, więc „0 komunikatów walidacji” wynika z braku wpisów, nie z licznika.
- Walidacja core **nie sprawdza** deskryptorów `UPDATE_AFTER_BIND` / `PARTIALLY_BOUND` przy dispatchu (tylko GPU-AV). W scenariuszu bindless deskryptory zapisano z `image_layout(GENERAL)`, a tłumaczenie przeniosło teksturę do `SHADER_READ_ONLY_OPTIMAL`: to niewykryte naruszenie specyfikacji. Zadziałało, bo AMD jest tolerancyjne. „0 komunikatów” nie dowodzi poprawności tego scenariusza.
- Shader bindless indeksuje stałymi (3 i 700); indeksowanie dynamiczne ani `NON_UNIFORM` nie zostało ćwiczone.

| Scenariusz | Wynik |
|---|---|
| Tekstura `Rgba8Unorm` 64×64 z 4 mipami, `TextureAccess` per mip (mip 0 `COLOR_ATTACHMENT`, mip 1 `SHADER_WRITE`) → `VkImageMemoryBarrier2` | działa |
| Jedno `TextureAccess` na 2 mipy w różnych stanach | backend rozbija na 2 bariery; wymaga własnej mapy stanów per (mip, warstwa) |
| Clear mipa 0 (dynamic rendering), compute zapisujący mip 1 do `StorageTexture`, kopia obu mipów do bufora i odczyt | piksele zgodne (`[255,128,0,255]`, `[0,255,0,255]`) |
| `SetBindGroup` przed `SetComputePipeline` | działa po odroczeniu bindowania do `Dispatch` |
| Timeline semaphore jako oczekiwanie na `submit` | działa |
| Bindless: 1024 sampled images, `PARTIALLY_BOUND` + `UPDATE_AFTER_BIND` + `VARIABLE_DESCRIPTOR_COUNT`, `binding_array` w WGSL, odczyt z indeksów 3 i 700 | odczyt działa, **ale** niewykryty błąd layoutu deskryptora (wyżej); layoutu nie da się wyrazić przez `BindGroupLayoutDesc` (surowy ash) |
| Zgodność formatów i limity | `D24_UNORM_S8_UINT` i `X8_D24` nieobsługiwane na tym GPU; ETC2 i ASTC niedostępne; `maxBoundDescriptorSets=32`, `maxPushConstantsSize=256` |

Pierwsze uruchomienie nie sprawdziło na GPU: trójkąta (potoku graficznego), próbkowania przez sampler, `present` i swapchaina, resize. Domknięto je 22.11 (sekcja niżej). Nadal niesprawdzone: depth, MSAA resolve, wiele kolejek, aliasing pamięci, `write_texture` / `write_buffer`, QuerySet, formaty skompresowane. Backend jest pominiętym szkicem: bez alokatora, bez pooli per ramka, bez bezpieczeństwa wątkowego (`&mut self` zamiast `&self`). Odczyt mipa 1 z `bytes_per_row` 128 przeszedłby w Vulkanie, ale wgpu go odrzuci (patrz #6d).

## Drugie uruchomienie (22.11.2026): wynik

Prototyp z 21.11 przepadł (leżał poza repo i nie dało się go odnaleźć), więc odtworzono nowy, równie wyrzucany: `ash 0.38`, `ash-window 0.13`, `winit 0.30`, `naga 30.0.2` (WGSL → SPIR-V), Vulkan 1.3 z `synchronization2`, `dynamic_rendering` i indeksowaniem deskryptorów. Ta sama maszyna: Windows 11, AMD Radeon RX 9070 XT (Vulkan 1.4.359, sterownik LLPC), Khronos validation 1.4.363. Tym razem **z debug messengerem i licznikiem** komunikatów ERROR / WARNING, w dwóch trybach: synchronization validation oraz GPU-assisted (GPU-AV). Kod nie wchodzi do repo.

Wynik: **23 sprawdzenia, 0 FAIL, 0 komunikatów ERROR** w obu trybach walidacji (po poprawce bariery z punktu 1 poniżej). Komunikaty WARNING (4 w trybie sync, 7 w GPU-AV) to wpisy loadera i walidatora o konfiguracji (nazwy warstw Galaxy Overlay, duplikat warstwy AMD, cechy wymuszane przez GPU-AV), nie naruszenia API; ich obecność dowodzi też, że messenger działa. Shadery przeszły `spirv-val --target-env vulkan1.3`.

| Scenariusz | Weryfikacja | Wynik |
|---|---|---|
| Clear mipa 0 (dynamic rendering) + compute zapisujący mip 1 przez storage image, bariery per mip | kopia obu mipów do bufora: `[255,128,0,255]` i `[0,255,0,255]` w każdym teksselu obu mipów | działa |
| Trójkąt: vertex buffer + index buffer, `draw_indexed`, dynamic rendering, cel 128×128 | piksele: wierzchołek `[246,3,6]`, lewy dół `[7,237,11]`, prawy dół `[7,8,240]`, tło `[0,0,0,255]` | działa |
| Konwencja WebGPU na Vulkanie (#6h), culling tylnych | trzy konfiguracje × `FrontFace` CCW / CW (tabela niżej) | **potwierdzona** |
| Tekstura 64×64, 4 mipy, upload przez staging (wiersze ciasne, `bufferRowLength` 0, bez wyrównania 256), per mip layouty `SHADER_READ_ONLY` / `GENERAL` / `SHADER_READ_ONLY` / `TRANSFER_SRC` w jednym obrazie | odczyt mipa 1 (layout `GENERAL`, 128 B na wiersz) i mipa 3 (`TRANSFER_SRC`) zgodny z danymi | działa |
| Sampler: liniowy i najbliższy, powiększenie 2× na granicy czarny / biały | liniowy: 64 i 191 (oczekiwane ~64 i ~191); najbliższy: 0 i 255 | działa |
| Sampler: wybór mipa z pochodnych (`textureSample`), `min_lod`, `max_lod`, widok tylko mipa 2 | cele 32², 16², 8² dają mip 1 / 2 / 3 (zielony / niebieski / żółty); `min_lod=1` daje mip 1; `max_lod=0` daje mip 0; widok `base_mip=2` czyta mip 2 | działa |
| Bindless: 1024 `SAMPLED_IMAGE`, `PARTIALLY_BOUND` + `UPDATE_AFTER_BIND`, **indeks nierównomierny** z `@builtin(position)` (8 różnych tekstur w jednym drawze), indeks bazowy z immediate (0 i 700), zapis deskryptorów 700..707 **po** `cmd_bind_descriptor_sets` i przed submitem; wariant ze stałą liczbą i z `VARIABLE_DESCRIPTOR_COUNT` | 8 kolumn w poprawnych kolorach w każdym z 4 przebiegów; layout deskryptora zapisany poprawnie (`SHADER_READ_ONLY_OPTIMAL`) | działa |
| Bindless, kontrola negatywna: indeks 900 (niezapisany) pod GPU-AV | `Descriptor index 900 is uninitialized` dla każdego fragmentu | GPU-AV działa |
| Swapchain + `present`, winit 0.30, 800×600, `FIFO`, 3 obrazy, `B8G8R8A8_UNORM` | odczyt obrazu swapchaina (kopia do bufora) w czterech punktach (800×600, 640×480, 1000×700 i po przywróceniu z minimalizacji): trójkąt na miejscu, tło czarne | działa |
| Resize 800×600 → 640×480 → 1000×700 (faza „reaktywna”: `Resized` ignorowany; faza „proaktywna”: `Resized` odtwarza swapchain), minimalizacja (inner size 0×0, 60 klatek pominiętych), przywrócenie | 270 klatek, 5 odtworzeń swapchaina, odczyt po każdym rozmiarze zgodny z rozmiarem okna; bez crashu i bez komunikatów walidacji | działa |

Konwencja współrzędnych (#6h): liczba pikseli trójkąta 5202 w każdej widocznej konfiguracji.

| Konfiguracja | `FrontFace` CCW + cull back | `FrontFace` CW + cull back |
|---|---|---|
| A: naga `ADJUST_COORDINATE_SPACE`, viewport normalny | widoczny, wierzchołek u góry | odrzucony |
| B: bez flagi, viewport z ujemną wysokością (jak wgpu-hal) | widoczny, wierzchołek u góry | odrzucony |
| C: bez flagi, viewport normalny | odrzucony | widoczny, **odwrócony w pionie** |

### Wnioski z uruchomienia

1. **Bariera po `vkAcquireNextImageKHR`.** Pierwsza wersja przejścia `UNDEFINED → COLOR_ATTACHMENT_OPTIMAL` miała `srcStageMask = NONE` i synchronization validation zgłosiła 10 błędów `WRITE_AFTER_READ` (obraz był wcześniej używany przez acquire). Poprawka: stage źródłowy musi obejmować etap, na którym submit czeka na semafor acquire (`COLOR_ATTACHMENT_OUTPUT` albo szerzej). W RHI to `texture_access(surface_texture, …, from: Access::empty(), to: …)`, więc regułę ma znać backend, nie paczka. Po poprawce 0 błędów.
2. **Resize na tej konfiguracji daje `SUBOPTIMAL`, nie `OUT_OF_DATE`.** W fazie reaktywnej `acquire` i `present` zwracały `SUBOPTIMAL_KHR` od klatki, w której zażądano zmiany rozmiaru; `ERROR_OUT_OF_DATE_KHR` nie wystąpił ani razu. Przy zignorowaniu `SUBOPTIMAL` stary swapchain 800×600 prezentował się poprawnie przez 80 kolejnych klatek (bez błędu). `OUT_OF_DATE` zaobserwowano tylko przy sztucznej próbie `acquire` na swapchainie zastąpionym (retired), gdzie walidacja słusznie ostrzega, że to niedozwolone. Wniosek: backend musi odtwarzać swapchain także po `SUBOPTIMAL` (po wyświetleniu klatki) i nie może polegać na `OUT_OF_DATE`; ścieżka `OUT_OF_DATE` pozostaje nieprzetestowana na prawdziwym zdarzeniu.
3. **Rozmiar swapchaina narzuca powierzchnia.** `currentExtent == minImageExtent == maxImageExtent` = rozmiar okna (nie `u32::MAX`), więc backend bierze rozmiar z możliwości powierzchni, a `SurfaceFrame.size` (już istnieje w `rhi-types`) niesie rozmiar rzeczywisty. Zminimalizowane okno: `inner_size` 0×0, `Resized(0×0)`, po przywróceniu `Resized` z poprzednim rozmiarem. `rhi-null` i `rhi-wgpu` zwracają wtedy `SurfaceUnavailable` (R0005); backend ash ma robić to samo (w spike'u po prostu pomija klatkę).
4. **Tryby prezentacji.** Ta powierzchnia oferuje `IMMEDIATE`, `FIFO`, `FIFO_RELAXED` i `FIFO_LATEST_READY`, **bez `MAILBOX`**. `PresentMode::LowLatency` mapuje się dziś na `Mailbox` i po cichu spada na `Fifo` (`present_mode_to_wgpu`), bez diagnostyki; ten sam spadek będzie w backendzie ash. Powierzchnia ma też `STORAGE` w `supportedUsageFlags`; format `B8G8R8A8_UNORM` nie jest pierwszy na liście (pierwszy to `R8G8B8A8_UNORM`).
5. **Bindless: walidacja nie widzi błędnego layoutu.** Ze zmienną środowiskową psującą layout (deskryptor `GENERAL`, obraz w `SHADER_READ_ONLY_OPTIMAL`): zwykły zestaw deskryptorów (scenariusz tekstury) dostaje błędy przy `vkQueueSubmit2` w obu trybach; zestaw bindless (`UPDATE_AFTER_BIND` + `PARTIALLY_BOUND`) dostaje **0 błędów** zarówno w trybie sync, jak i w GPU-AV. Potwierdza to regułę z #2: layout deskryptora ma być deterministyczną funkcją typu bindingu i formatu, bo żadna walidacja go nie pilnuje. GPU-AV łapie za to niezapisane deskryptory (kontrola negatywna wyżej).
6. **`ShaderNonUniform` tylko dla indeksu nierównomiernego.** naga dopisuje `OpCapability ShaderNonUniform` i dekoracje `NonUniform` tylko wtedy, gdy indeks tablicy jest nierównomierny wg jego analizy (indeks z `@builtin(position)`); indeks liczony wyłącznie z immediate nie dał ani capability, ani dekoracji. Backend musi więc włączać `shaderSampledImageArrayNonUniformIndexing` zgodnie z `Features::NON_UNIFORM_INDEXING`, a shader z nierównomiernym indeksem bez tej cechy powinien dostać diagnostykę przed tworzeniem potoku.
7. **SPIR-V 1.3 vs GPU-AV.** Moduł z dwoma entry pointami (vs i fs) wygenerowany jako SPIR-V 1.3 daje przy GPU-AV ostrzeżenie `Found 2 different OpVariable, can't determine which entrypoint`; przy 1.4 ostrzeżenia nie ma. Backend ash ma ustawić `lang_version` naga na (1, 4) (Vulkan 1.3 to dopuszcza). GPU-AV wykrył niezapisany deskryptor także przy 1.3, ale instrumentacja jest wtedy mniej pewna.
8. **Limity deskryptorów.** `VARIABLE_DESCRIPTOR_COUNT` też działa, więc decyzja ADR-0017 „stała liczba” to wybór, nie konieczność. Zestawy z `UPDATE_AFTER_BIND` podlegają osobnym limitom (`maxPerStageDescriptorUpdateAfterBind*`); AMD zgłasza `u32::MAX` dla wszystkich, więc nie sprawdzono różnicy. Skoro bind groupy w RHI są niemutowalne (ADR-0017), backend nie potrzebuje `UPDATE_AFTER_BIND`; do zweryfikowania w M9-01 (potrzebne jest tylko `PARTIALLY_BOUND`).

### Czy poprawki M2-20…M2-24 wystarczają

Tak, dla wszystkiego, co uruchomiono. Stan docelowy plus poprzedni z `TextureAccess`, podział `Access`, tablice bindingów ze stałym `count`, `TextureViewArray`, `PARTIALLY_BOUND`, `NON_UNIFORM_INDEXING` i immediates dają się odwzorować na Vulkan bez zmiany typów w `pathon-rhi-types`. Różne layouty per mip w jednym obrazie oraz widoki ograniczone do zakresu mipów działają i przechodzą walidację, więc mapa stanów per (mip, warstwa) w backendzie jest wykonalna. Kopie bufor↔tekstura z ciasnymi wierszami (128 B, 32 B) przechodzą w Vulkanie (wymóg 256 B to ograniczenie wgpu, patrz #6d). Nowe luki nie zmieniają typów; to reguły backendu ash i dwie drobne poprawki w dokumentacji i diagnostyce.

### Dodatkowe poprawki (propozycje, nie zaimplementowane)

| Numer | Zakres | Źródło |
|---|---|---|
| M2-25 (nowy) | Diagnostyka spadku trybu prezentacji (`LowLatency` → `Vsync`, gdy brak `Mailbox`) w `rhi-wgpu` oraz sekcja „Powierzchnia” w `docs/crates/pathon-rhi-types.md` (zerowy rozmiar → `SurfaceUnavailable`, `Suboptimal` → odtworzenie po prezentacji, rozmiar z `SurfaceFrame.size`). Bez zmiany typów | wnioski 2–4 |
| M9-01 (lista kontrolna backendu) | Przejście z `Access::empty()` na teksturze z `acquire_surface` z etapem źródłowym acquire (wniosek 1); odtwarzanie swapchaina po `SUBOPTIMAL`; semafor „render finished” per obraz swapchaina; viewport z ujemną wysokością zamiast `ADJUST_COORDINATE_SPACE`; naga `lang_version` (1, 4); `shaderSampledImageArrayNonUniformIndexing` zgodnie z `NON_UNIFORM_INDEXING`; stały layout deskryptorów wg #2; `PARTIALLY_BOUND` bez `UPDATE_AFTER_BIND` | wnioski 1, 2, 5–8 oraz #6h |
| M9-03 (zgodność) | Test z walidacją GPU-AV: odczyt przez bindless po nietrywialnej sekwencji `TextureAccess` (golden pixels), bo layoutu bindless nie pilnuje walidacja core; kontrola negatywna z niezapisanym deskryptorem | wniosek 5 |
| M3-02 / M3-03 (sprawdzić) | Czy shader z indeksem nierównomiernym, utworzony bez `Features::NON_UNIFORM_INDEXING`, dostaje diagnostykę z kodem (dziś polega na walidatorze naga w wgpu); `rhi-null` powinien zachowywać się tak samo | wniosek 6 |

Bez zmian w kolejności z „Kolejność przed M3”; nowe pozycje nie blokują M3 ani zamrożenia w M5-07.

## Werdykty i zmiany w `pathon-rhi-types`

Werdykt: **zostaje** / **koryguję** / **później** / **odpada**. „Łamiąca” = łamie konstruktory lub kod paczek.

### Przed M3 (zadania M2-20…M2-24)

| # | Werdykt | Zmiana w poprawnym kształcie | Łamiąca |
|---|---|---|---|
| 1 Tablice bindingów | koryguję | `BindGroupLayoutEntry::count: Option<NonZeroU32>`; `BindingResource::TextureViewArray(Vec<…>)` i `SamplerArray` (+ bufory), limit `max_binding_array_sampler_elements_per_shader_stage`. **Bez** `VARIABLE_DESCRIPTOR_COUNT` i reguły „tablica ostatnia” (naga emituje stałe `binding_array<T, N>`; wgpu-hal nie używa zmiennej liczby). Refleksja (M3-02) wymusza regułę wgpu: grupa z tablicą nie zawiera uniform buffera ani bufora z dynamicznym offsetem (`ContainsBothBindingArrayAndUniformBuffer`). Publiczna flaga `UPDATE_AFTER_BIND` bez API aktualizacji jest bez sensu (bind groupy wgpu są niemutowalne): zdecydować „odtwórz grupę” vs `update_bind_group_element` z fallbackiem na wgpu. Godot: wiele `add_id` na binding (stały rozmiar), bez partially bound | tak: pole w literałach; `Vec` odbiera `Copy` z `BindingResource` i `BindGroupEntry` |
| 2 Podział `SHADER_READ` | zostaje, wzmacniam | `SAMPLED`, `STORAGE_READ`, `STORAGE_WRITE`; `SHADER_READ` **nie** zostaje jako suma. Layout deskryptora to deterministyczna funkcja typu bindingu i formatu, bo jest wpisywany przy `create_bind_group`, zanim znany jest jakikolwiek `Access`: kolor `SHADER_READ_ONLY_OPTIMAL`, depth `DEPTH_STENCIL_READ_ONLY_OPTIMAL`, storage `GENERAL`. `is_valid_state`: `SAMPLED` łączy się tylko z `DEPTH_READ`; odrzucić `SAMPLED\|COPY_SRC` i `SAMPLED\|STORAGE_READ`. **Żywy błąd na wgpu:** `usage_for` w `crates/pathon-graph/src/compile.rs` mapuje `SHADER_READ` tylko na `TextureUsage::SAMPLED`, więc pass czytający teksturę storage tylko-do-odczytu nie dostaje `STORAGE`, o ile inny pass jej nie zapisuje. Poprawka: `STORAGE_READ` → `TextureUsage::STORAGE`; walidacja w `rhi-null` | semantycznie (zmiana bitów) |
| 5 Immediates i overrides | koryguję | Słownictwo wgpu 30: `Features::IMMEDIATES`, `Limits::max_immediate_size`, `set_immediates(offset, data)` bez etapu, WGSL `var<immediate>`; w przeglądarkach jeszcze niedostępne, więc paczki potrzebują fallbacku na Web. Opis potoku: `immediate_size: u32` bez etapu. Godot `*_list_set_push_constant(list, data, size)` nie ma offsetu: kontrakt całego bloku. `Command::SetImmediates` musi zmieścić się w limicie 32 B (ADR-0009): box albo arena boczna. Specjalizacja to **nazwane overrides `f64`** rozwiązywane przy kompilacji naga (naga → SPIR-V odrzuca nierozwiązane override, `Error::Override`; wgpu przyjmuje `constants: &[(&str, f64)]`), nie `VkSpecializationInfo`. `Vec` w `ShaderStage` odbiera `Copy` | tak: pola, utrata `Copy`; nowy wariant `Command` jest `non_exhaustive` |
| 6 Kontrakty | zostaje, rozszerzam, poprawiam (d) | Dokument + ADR (M2-20). (a)–(c), (e), (f) jak niżej. **(d) poprawione:** 256 B to `COPY_BYTES_PER_ROW_ALIGNMENT` tylko dla kopii bufor↔tekstura; `write_texture` jej nie ma. **(b) zdecydować teraz:** `TextureAccess` niesie tylko stan docelowy, co wymusza mapę stanów per (mip, warstwa) i tłumaczenie sekwencyjne, sprzeczne z równoległym nagrywaniem passów, które zostawia otwarte ADR-0012. Poprzedni stan idzie do komendy (tanie teraz, paczki nie konstruują `TextureAccess`). Nowe: **(g)** które metody `DeviceApi` wolno wołać poza wątkiem renderu / współbieżnie z `submit` (M3-04: kompilacja w tle woła `create_*_pipeline` z innych wątków; backend Godota musi marshalować). **(h)** NDC i winding jak w WebGPU; viewport Godota jest stały od `draw_list_begin`, więc flip Y przez `ADJUST_COORDINATE_SPACE` w naga (**zweryfikowane 22.11**: obie metody dają obraz WebGPU, patrz „Drugie uruchomienie”). **(i)** brak dynamicznych offsetów uniformów. **(j)** polityka rozrostu struktur (patrz niżej). (a) po `write_*` zasób zachowuje dane, a `Access::empty()` (UNDEFINED) je kasuje, więc backend pamięta stan po uploadzie; (c) `SetBindGroup` dozwolone przed `Set*Pipeline`; (e) `write_*` uporządkowane względem następnego `submit`; (f) `destroy` zasobu użytego w zgłoszonej liście jest odroczone | nie (dokument); poprzedni stan w komendzie: tak |
| P Pipeline cache (brak w spike'u) | nowe | M3-05: `Features::PIPELINE_CACHE` istnieje, ale `DeviceApi` nie umie wczytać ani pobrać blobu. Blob w `DeviceRequest` (globalny dla urządzenia) + getter | tak (pole w `DeviceRequest`) |
| S Źródło shaderów (brak w spike'u) | nowe | Co dostaje `create_shader_module`: `ShaderModuleDesc` jest tylko WGSL. Zdecydować przed M3-01: wariant SPIR-V czy WGSL wszędzie (Godot i ash chcą SPIR-V). Krótki ADR | zależy od decyzji |
| K Klucz potoku (brak w spike'u) | nowe | Stan zmieniający klucz permutacji w M3-03: bias głębi (M4-07), stan stencila, unclipped depth i polygon mode (bity `Features` bez pól), alpha-to-coverage (M4-19). Ustalić listę pól przed M3-03 | tak (pola) |
| J Polityka rozrostu struktur (brak w spike'u) | nowe | Źródło każdego „łamiąca: pole w literałach”. Albo konstruktory i buildery (`#[non_exhaustive]` + `new`), albo akceptacja zmian do zamrożenia w M5-07. Decyzja w ADR z M2-20 | n/d |

### Przed M4-06 (termin twardy: M5-07)

| # | Werdykt | Zmiana | Łamiąca |
|---|---|---|---|
| 4 `view_formats` / cube | koryguję, przesuwam | `Vec<Format>` zepsułoby `TextureDesc: Copy` i `same_allocation` (pooling): użyć `&'static [Format]` albo flagi widoku sRGB. Vulkan wnioskuje kompatybilność z cube (`desc.is_cube_compatible()` w wgpu-hal); Godot jej wymaga (`texture_type` cube). Propaguje się do zamrożonego `GraphTextureDesc` (brak cube / wymiaru / liczby próbek). Potrzebne dla IBL. **Zrobione 22.11.2026** (bez `GraphTextureDesc`): `view_formats: &'static [Format]`, cube przez widok, aneks w ADR-0020 | tak (pola) |
| 13 Godot: offsety draw, viewport | koryguję, później (M7-01) | Każde nowe pole `pub` w `BufferDesc` łamie literały (to nie „pole opcjonalne”). Godot miesza użycia bufora (`BUFFER_CREATION_AS_STORAGE_BIT`, storage + indirect), więc kombinacje nie są problemem. `first_instance` można emulować przez `draw_list_draw_indirect`. `Features::DRAW_BASE_OFFSETS`, `DYNAMIC_VIEWPORT`, `BufferDesc::index_format` | tak (pole) |
| Indirect (M4-11) | nowe | Jest tylko `DrawIndexedIndirect`; brak `DrawIndirect`, `DispatchIndirect`, `MultiDrawIndexedIndirect` i wariantów `…Count`. Wariant z count (~36 B) przekracza limit 32 B bez boxowania; Godot nie ma wariantu count. Powtórzyć benchmark z ADR-0009. **Zrobione 22.11.2026** (część RHI M4-11): komendy, typy argumentów, walidacja null i test GPU; wariant `…Count` w `Box<MultiDrawCount>`, `Command` zostaje 32 B, brak bitu `MULTI_DRAW_INDIRECT` ([ADR-0033](adr/0033-komendy-indirect.md)) | nie (nowe warianty) |
| Bindless na M4-04 (z #1) | nowe | Model aktualizacji, reguła uniform buffera, fallback na Web | n/d |

### Później

| # | Werdykt | Uwagi | Kiedy |
|---|---|---|---|
| 3 `SubmissionIndex` | później (zdegradowane) | Brak konsumenta w M3; `wait` nie zablokuje na wasm; globalne urządzenie Godota nie ma `submit`. Formalnie łamiące (typ zwracany), ale nie dla kodu paczek | najpóźniej M8/M9 |
| 7 Etapy w `Access` | później | Niełamiące tylko jako nowy wariant (nowe pole w `Command::TextureAccess` łamie konstruktory) | wydajność |
| 8 `QueueKind` | później | Domyślnie `Graphics`; zależności przez `SubmissionIndex` | M8-04 |
| 9 Discard / aliasing | później | Pokrywa się z #6(a); `Command::DiscardTexture` lub flaga; Godot `texture_set_discardable` | M9 |
| 10 `Depth24PlusStencil8` | obserwacja prawdziwa, **naprawa błędna** | Vulkan gwarantuje jedno z `D24S8` / `D32FS8`. Zostaje format abstrakcyjny, mapowany spójnie w teksturze i potoku (jak wgpu-hal); `texture_desc()` **nie** zwraca „faktycznego” formatu. Bez zmiany API | M9-01 |
| 11 Kopie bufor↔tekstura w Godocie | później | Bit „zakazane w paczkach” złamałby przenośność; preferować emulację w backendzie. Decyzja w M7-00 | M7-00 |
| 12 Powierzchnie i import | później | Godot ma `texture_create_from_extension` (import natywnego obrazu): dopisane do mapowania. Formalnie łamiące (rozdzielenie traitu), nie dla paczek | M7-00 |
| 14 Timestampy | zostaje | W Godocie `TIMESTAMP_QUERY` wyłączone, profil z hosta | M7-01 |
| 15 Wątki | zostaje | Powiązane z #6(g) i M3-04; backend Godota owija uchwyt z asercją wątku albo trait rozluźnia wymóg (ADR w M7-00) | M7-00 |
| 16 Dodatkowe `Features` / `Limits` | w większości odpada | `BUFFER_DEVICE_ADDRESS` bez konsumenta; `Limits` jest wyczerpującą strukturą `pub`, więc dodanie pól formalnie łamie | przed zamrożeniem albo poza zamrożoną powierzchnią |

### Które zmiany łamią kontrakt paczek: termin M5-07

- **Muszą wejść przed M5-07:** #1, #2, #4, #5, #13 oraz polityka rozrostu struktur (J).
- **Formalnie łamiące dla `semver-checks`, nie dla kodu paczek:** #3, #12, #16. Wejść przed zamrożeniem albo wyłączyć z zamrożonej powierzchni implementowalność `DeviceApi` i konstrukcję `Limits`.
- **Addytywne po zamrożeniu:** #7, #8, #9 (nowe warianty), #11, #14, nowe bity `Features`.

Kolejność przed M3: (1) dokument kontraktów z decyzjami, (2) podział `Access`, (3) tablice bindingów, (4) immediates i overrides, (5) pipeline cache i decyzja o źródle shaderów.

## Czego nie trzeba zmieniać (część nieprzetestowana)

Sprawdzone prototypem:

- `Command` z `TextureAccess` per podzasób mapuje się 1:1 na `VkImageMemoryBarrier2` (z mapą stanów w backendzie); w Godocie jest jedynie walidowane, bo `RenderingDevice` śledzi stany sam (to drugie: papier).
- Shadery WGSL (ADR-0006) przez naga → SPIR-V działają. `binding_array` wymaga w WGSL `enable wgpu_binding_array;`, więc takie shadery nie są przenośne do WebGPU i muszą być bramkowane przez `TEXTURE_BINDING_ARRAY`.

**Wnioski z samego czytania (nieprzetestowane):**

- Opisy samplerów, formaty wierzchołków, blend i `Features` jako flagi tłumaczą się wprost.
- `ExternalCommand` z `&mut dyn Any` pasuje do `VkCommandBuffer` i uchwytu `RenderingDevice`; w prototypie `Command::External` wpadło w gałąź „niemapowane”.
- Pula deskryptorów nie wymaga pól w `BindGroupDesc`. W `run.log` prototyp zgłosił to jako GAP (rozmiary puli zgadywane: stałe 64 na typ); **odwracam ten wniosek**: rozmiar puli jest sprawą backendu (pule rosnące, per ramka), a nie opisu. Odwrócenie jest poprawne, ale to ocena, nie wynik testu.

## Mapowanie RHI v1 na Godot `RenderingDevice`

Źródło: dokumentacja klasy `RenderingDevice` i klas `RDUniform`, `RDTextureFormat`, `RDTextureView`, `RDFramebufferPass` (docs.godotengine.org/en/stable). **Wersja dokumentacji (wcześniej „Godot 4.7”) nie została zweryfikowana.** Opisy metod przeczytano ogólnie; semantyki szczegółów nie sprawdzono w działającym Godocie. Całość jest papierowa.

| pathon RHI v1 | Godot `RenderingDevice` | Uwagi |
|---|---|---|
| `create_buffer` (`BufferUsage`) | `vertex_buffer_create`, `index_buffer_create`, `uniform_buffer_create`, `storage_buffer_create` (+ `BufferCreationBits`) | Rodzaj bufora wybierany przy tworzeniu; mieszanie użyć możliwe (`BUFFER_CREATION_AS_STORAGE_BIT`); format indeksów z góry (#13) |
| `write_buffer`, `read_buffer` | `buffer_update`, `buffer_get_data`, `buffer_get_data_async` | `MAP_READ` bez odpowiednika |
| `create_texture` | `texture_create(RDTextureFormat, RDTextureView)` | `TextureUsage` → `TextureUsageBits`; cube jako `texture_type` (#4) |
| `create_texture_view` | `texture_create_shared_from_slice` / `texture_create_shared` | Zakres mipów i jedna warstwa lub typ plastra; brak widoku „stencil only” |
| `write_texture` | `texture_update(texture, layer, data)` | Dane całej warstwy; `TexelLayout` bez wpływu |
| `create_sampler` | `sampler_create(RDSamplerState)` | Zgodne |
| `create_shader_module` + `*PipelineDesc` | `shader_create_from_spirv(RDShaderSPIRV)` + `render_pipeline_create` / `compute_pipeline_create` | Shader Godota łączy etapy; moduły pathon scalić przy tworzeniu potoku; wymaga SPIR-V (decyzja S) |
| `BindGroupLayout` | brak osobnego obiektu | Layout wynika z refleksji shadera |
| `BindGroup` | `uniform_set_create(uniforms, shader, set)` | Wymaga shadera i numeru zestawu: materializacja leniwa przy pierwszym użyciu z potokiem albo shader-stub z layoutu |
| `Command::TextureAccess` / `BufferAccess` | brak (automatyczne bariery RD); `barrier`, `full_barrier` zbędne | `Access` tylko walidowany (M7-01) |
| `BeginRenderPass` | `framebuffer_create` + `draw_list_begin(framebuffer, draw_flags, clear_colors, depth, stencil, region)` | `LoadOp::Clear` = `DRAW_CLEAR_COLOR_n`, `DontCare` = flaga ignorowania; `StoreOp` bez odpowiednika (#13); jedna lista naraz |
| `BeginComputePass` / `Dispatch` | `compute_list_begin`, `compute_list_bind_compute_pipeline`, `compute_list_bind_uniform_set`, `compute_list_dispatch(_indirect)`, `compute_list_add_barrier` | Zgodne |
| `SetBindGroup` | `*_list_bind_uniform_set(list, set, index)` | |
| `SetVertexBuffer`, `SetIndexBuffer`, `Draw*` | `vertex_array_create`, `index_array_create`, `draw_list_bind_*`, `draw_list_draw`, `draw_list_draw_indirect` | Zakresy i `base_vertex` przez RID (#13) |
| `SetViewport`, `SetScissor` | `region` w `draw_list_begin`; `draw_list_enable_scissor` | Viewport dynamiczny niedostępny; flip Y (#6h) |
| immediates (#5) | `*_list_set_push_constant(list, data, size)` | Bez offsetu: cały blok |
| `Transfer::BufferToBuffer`, `TextureToTexture` | `buffer_copy`, `texture_copy` | |
| `Transfer::BufferToTexture`, `TextureToBuffer` | brak | #11 |
| `QuerySet`, `PassTimestamps` | `capture_timestamp`, `get_captured_timestamp_*` | #14 |
| `PushDebugGroup` | `draw_command_begin_label` / `end_label` | |
| `Command::External` | wywołanie na uchwycie `RenderingDevice` | |
| Powierzchnie, import | `draw_list_begin_for_screen`, `screen_get_*` (tylko okno silnika); `texture_create_from_extension` (import natywnego obrazu) | W Godocie pathon renderuje do tekstur hosta (`CompositorEffect`, `Texture2DRD`), więc #12 |
| `Caps` | `limit_get`, `has_feature`, `texture_is_format_supported_for_usage`, `get_device_name` | Limity per typ w zestawie, nie per grupa |
| `submit`, `wait_idle` | globalne urządzenie: silnik zgłasza sam; lokalne: `submit()` + `sync()` | #3, #15 |
| Formaty | `DataFormat` obejmuje BC, ETC2, ASTC | Bez abstrakcyjnego `Depth24Plus` (#10) |

Najpoważniejsze utrudnienia dla M7-01: #1 (tablice), #11 (kopie bufor-tekstura), #12 (powierzchnie i import tekstur hosta), #13 (offsety draw i viewport) oraz leniwe tworzenie `uniform_set`. Żadne nie blokuje backendu, jeśli trafią do `Features` przed implementacją.

## Czego spike nie pokrył

- Depth i MSAA (potok graficzny, próbkowanie, swapchain, resize i `present` zostały domknięte 22.11).
- Przejście `wgpu` → `ash` w tym samym teście konformancji (to M9-03); pomiar wydajności na innych sterownikach.
- Wiele kolejek, async compute, własność zasobów między kolejkami, aliasing pamięci, alokator pamięci GPU.
- `write_texture` / `write_buffer`, QuerySet, formaty skompresowane, wiele warstw tablicy (tylko barierki w kodzie, bez uruchomienia).
- Godot: wszystko papierowo, bez uruchomionego silnika; tylko dokumentacja API (wersja niezweryfikowana).
- Wydajność (liczba barier, koszt mapy stanów), zachowanie na innych sterownikach niż AMD.
