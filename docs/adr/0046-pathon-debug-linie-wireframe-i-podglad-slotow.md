# ADR-0046: `pathon-debug` — linie, wireframe i podgląd slotów

**Status:** Przyjęta
**Data:** 2026-11-25
**Dotyczy:** M4-14, [ADR-0012](0012-kontrakt-paczki-w-pathon-api.md) (kontrakt paczki), [ADR-0019](0019-pipeline-cache-i-zrodlo-shaderow.md) (pole `polygon_mode` w kluczu potoku), [ADR-0022](0022-zegar-klatki-paczek.md) (klatka i próby nagrania), [ADR-0025](0025-golden-images.md) (golden images), [ADR-0041](0041-pack-lib.md) (granice zależności paczek), [docs/slots.md](../slots.md)

## Kontekst

Pierwsze paczki produkują już sloty (`depth`, `normals`, `motion`, `hdr`…), a nic nie pozwala ich zobaczyć ani narysować na nich czegokolwiek: błąd w kodowaniu normalnych albo wektorach ruchu widać dopiero po złym obrazie końcowym. Potrzebne są linie i gizmo w przestrzeni świata, ramki AABB, wireframe i podgląd dowolnego slotu bazowego. RHI nie ma `polygon_mode` (ADR-0019 zapowiedział go „z pierwszym użytkownikiem”), a Web nie ma linii sprzętowych. Paczka nie może zależeć od niczego poza `pathon-api` i `pathon-shader`, więc nie zna scen ani materiałów innych paczek.

## Decyzja

### Jedna paczka, trzy `RenderFeature`, zależność tylko od `pathon-api`

`DebugPack` rejestruje `SlotPreviewFeature` (priorytet 0), `WireframeFeature` (10) i `LinesFeature` (20), wszystkie w `Stage::Overlay` po tonemappingu, więc kolejność w grafie to podgląd → wireframe → linie na `Slot::OUTPUT`. Każdą można zarejestrować osobno. `DebugDraw` (dane) i `DebugSettings` (ustawienia) leżą w `Shared`; aplikacja zmienia je między klatkami. Paczka nie zna żadnej innej paczki: z producentami slotów łączy ją kontrakt slotów.

### `DebugDraw`: tryb natychmiastowy, w pamięci, bez I/O

Linie, łamane, AABB, sześcian z macierzą (OBB), okręgi, sfery (trzy okręgi), strzałki, gizmo osi (X czerwony, Y zielony, Z niebieski), ostrosłup kamery i siatki do wireframe. Wszystko we współrzędnych świata, wszystko to `Vec` linii: nic tu nie dotyka GPU.

- **Historia klatek to żywotność.** `LineStyle::persistent(n)` rysuje prymityw przez `n` klatek (domyślnie 1). Licznik spada dopiero, gdy klatka naprawdę pokazała prymityw (`ViewCtx::frame`, nie próba nagrania: ADR-0022), więc ponowne nagranie tej samej klatki niczego nie zjada, a linia dodana po klatce nie jest starzona przez klatkę, której nie widziała.
- **Limity** (domyślnie 262 144 linii i 65 536 trójkątów wireframe): nadmiar jest odrzucany w całości i zgłaszany raz jako U0002; siatka wireframe nie jest ucinana w połowie.
- Nieskończone współrzędne są pomijane, kolory i grubości naprawiane (przycięcie do 0…1 i 0.1…64 px).
- **Tekst jest poza zakresem.** Napisy potrzebują atlasu fontów, kształtowania i zależności, które nie należą do paczki diagnostycznej bez zależności; należą do UI hosta albo osobnej paczki. Konsekwencja: gizmo i podglądy nie mają podpisów, a siatka podglądu ma kolejność z listy.

### Linie: czworokąty w przestrzeni ekranu

Odcinek to jedna instancja i sześć wierzchołków; vertex shader przycina odcinek do płaszczyzny bliskiej (`w > 1e-4`) i rozszerza go do czworokąta w pikselach, fragment liczy odległość od odcinka (zaokrąglone końce, rampa antialiasingu 1 px). Dzięki temu grubość jest w pikselach niezależnie od odległości i skali renderu, a test GPU porównuje obraz z wartościami `reference::segment_coverage` (pozycja i grubość).

- Dane idą w buforze wierzchołków z krokiem per instancja, nie w storage bufferze: bufor storage w vertex shaderze nie jest gwarantowany na każdym profilu WebGPU, a offsety wierzchołków nie wymagają wyrównania 256.
- **Bufor GPU** (`VertexArena`) rośnie przez podwajanie, a każdy widok tej samej próby nagrania dostaje własny zakres (restart przy zmianie `record_epoch`); wymieniony bufor idzie do `RecordCtx::release_later`.
- **Test głębi w shaderze**, nie w attachmencie: `Slot::DEPTH` ma rozmiar renderu, a `output` rozmiar wyjścia (G0004 zabrania mieszania ich w jednym passie), więc fragment czyta głębię przez `textureLoad` pod przeliczonym pikselem i porównuje z tolerancją względną (domyślnie 0.1%, reverse-Z). Bez slotu `depth` linia z `depth_tested` jest po prostu rysowana.
- **Kolor** to to, co ma być widać (sRGB). Cele, które dekodują bajty jako liniowe (`*Srgb`, zmiennoprzecinkowe), dostają kolor zdekodowany do liniowego (`LINEARIZE`, jak OETF w tonemapperze, odwrotnie), więc obraz jest ten sam na każdym celu; mieszanie to alpha premultiplied.

### `PolygonMode` w RHI

`PrimitiveState::polygon_mode: PolygonMode { Fill, Line }` (domyślnie `Fill`). `Point` dojdzie z `Features::POLYGON_MODE_POINT` razem z pierwszym użytkownikiem. `Features::POLYGON_MODE_LINE` i jego mapowanie na wgpu już istniały; dochodzi:

- `PrimitiveState::check_features(&Caps)`, wołane przed utworzeniem potoku w `rhi-wgpu` i `rhi-null`: `Line` bez funkcji to `RhiError::MissingFeatures(POLYGON_MODE_LINE)` na każdym backendzie, a nie panika wgpu;
- test konwersji (`polygon_modes_map_to_the_wgpu_ones_and_line_to_its_feature`) i test walidacji w `rhi-null`;
- pole jest częścią `RenderPipelineDesc: Eq + Hash`, więc wchodzi do klucza potoku samo (ADR-0019, tabela pól: pozycja `polygon_mode` zrealizowana).

### Wireframe: dwie ścieżki, jeden bufor

`DebugDraw::wireframe(positions, indices, transform, style)` zamienia siatkę na świat (trójkąty bez indeksów, każdy ze swoimi trzema wierzchołkami z kolorem i grubością). `WireframeFeature` rysuje je jedną z dwóch ścieżek:

| Ścieżka | Warunek | Wygląd |
| --- | --- | --- |
| polygon mode `Line` | `POLYGON_MODE_LINE` (paczka prosi o niego jako opcjonalny w `requirements()`) i `WireframeMode::Auto` | linie sprzętowe 1 px, bez antialiasingu |
| barycentryczna | brak funkcji (Web) albo `WireframeMode::Barycentric` | wypełnione trójkąty; fragment zostawia piksele bliskie krawędzi (`fwidth` współrzędnych barycentrycznych), z antialiasingiem i grubością |

Brak funkcji to `Negotiation::Fallback` (F0103), nie osobna diagnostyka. Ścieżka barycentryczna rysuje tylko wnętrze trójkąta: krawędź wspólna ma pełną grubość (dwie połowy), krawędź brzegowa siatki połowę. Dlatego „identyczny wizualnie” znaczy w teście: każdy piksel linii sprzętowej ma piksel fallbacku w odległości do dwóch pikseli i odwrotnie, fallback ma od 0.7 do 1.6 razy tyle pikseli linii co ścieżka sprzętowa, oba obrazy mają golden z własnym progiem (linie sprzętowe różnią się między backendami o piksel, więc golden ma próg 100 / 10%). Test sprawdza też `barycentric_coverage` dla grubości 4 px kolumna po kolumnie.

### Podgląd slotów

`SlotPreview { slots, layout, … }` z układami `Fullscreen` (czyści cel), `Corner(Corner)` (część szerokości celu, na tym, co jest) i `Grid` (kafelki w kolejności listy na ciemnym tle). Slot podglądu jest typem `PreviewSlot` ze wszystkimi trzynastoma slotami bazowymi poza `output`: ten, do którego rysuje podgląd, nie ma sensu pokazywać.

| Slot | Wizualizacja |
| --- | --- |
| `depth`, `depth_pyramid` | odległość widokowa z odwrotności projekcji kamery (działa dla każdej projekcji reverse-Z), rampa `heat` w `depth_range`; piramida: mip `pyramid_level` |
| `normals` | oktaedralne → wektor → `n·0.5+0.5` |
| `motion` | odcień = kierunek, jasność = długość w px do `motion_scale` |
| `material_id` | kolor z hasha PCG |
| `roughness`, `reactive`, `ao` | skala szarości |
| `albedo` | liniowe → sRGB |
| `hdr`, `diffuse`, `specular` | `× exposure` (ustawienie albo slot `exposure`), obcięte, sRGB; NaN i Inf → magenta |
| `exposure` | `log2` na rampie `heat` |

Wszystko przez `textureLoad`, więc źródło nie musi być filtrowalne. Trzy moduły shadera według rodzaju tekstury (float, głębia, uint) składa się z tych samych części WGSL, plus czwarty (tylko uniform) dla kafelka „brak slotu”.

**Kontrakt z producentami.** Podgląd niczego nie produkuje. Slot na żądanie zamawia w `prepare_view` (tak jak `OnDemandConsumer` w `pathon-test`, M2-14), a slot, którego nikt nie zapisał, pokazuje jako szachownicę z diagnostyką U0001 raz, dopóki nie wróci. To rozstrzyga pytanie „czy debug wymusza produkcję”: zamawia, nie wymusza.

### Diagnostyki

| Kod | Znaczenie |
| --- | --- |
| U0001 | slot do podglądu niedostępny w tej klatce |
| U0002 | `DebugDraw` pełny, prymitywy odrzucone |
| U0003 | nieprawidłowe ustawienia podglądu (naprawione) albo pusta lista slotów |

### Atrapa producenta i testy

`SyntheticSlots` w `pathon-test` zapisuje każdy slot bazowy znanym wzorem (`synthetic_slots.wgsl`), z zachowaniem kontraktu na żądanie; służy testom i przykładowi `debug_slots`, bo producent pełnego zestawu (pbr) jeszcze nie pisze wszystkiego. Przykład zapisuje do PNG podgląd każdego slotu, siatkę i scenę z liniami.

Testy: jednostkowe i na `rhi-null` (walidacja barier i bind groups bez GPU), GPU (`tests/gpu.rs`): linie względem matematyki shadera, grubość niezależna od odległości, AABB wzdłuż rzutowanych krawędzi, test głębi, każdy podgląd względem wzoru i golden, siatka, kafelek brakującego slotu, `Rgba8UnormSrgb` kontra `Rgba8Unorm`, obie ścieżki wireframe. Referencje: `tests/golden/debug_*.png` (Vulkan, RX 9070 XT; ten sam zestaw przechodzi na DX12).

## Konsekwencje

- Zmiana w RHI jest addytywna dla kodu, który buduje `PrimitiveState` przez `..Default::default()` (cały dotychczasowy kod).
- Wireframe rysuje siatki zgłoszone do `DebugDraw`, a nie sceny pbr: do czasu wspólnego opisu list rysowania (host dostarcza trójkąty) paczka nie zna cudzych siatek. Gdy pbr da widok na swoje siatki, `WireframeFeature` dostanie drugie źródło bez zmiany ścieżek rysowania.
- Grubość linii sprzętowych jest zawsze 1 px; kto chce grubszych, używa ścieżki barycentrycznej (`WireframeMode::Barycentric`).
- Poza zakresem zostają: tekst, `PolygonMode::Point`, wireframe z głębią przez attachment (wymagałoby tego samego rozmiaru co `output`), przestrzenie barw HDR celu (cel zmiennoprzecinkowy dostaje liniowy sRGB, bez nitów).
- Referencje na lavapipe, WARP i Metalu powstaną dopiero w CI; progi golden dla wireframe są szersze przez rasteryzację linii.
