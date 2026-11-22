# ADR-0027: Pula geometrii, layouty wierzchołków i dane poprzedniej klatki

**Status:** Przyjęta
**Data:** 2026-11-22

## Kontekst

Forward+, visibility buffer (M8-02) i ray tracing (M8-03) czytają tę samą geometrię, a każdy robi to inaczej: forward przez bufory wierzchołków i indeksów, visibility buffer i RT przez vertex pull ze storage, GPU culling (M4-11) przez `draw_indexed_indirect` z jednego bindu. Wektory ruchu (kontrakt slotów, M2-10) potrzebują pozycji z poprzedniej klatki, także dla skinningu (M4-12), który zapisuje pozycje w compute. Layout tych danych jest najdroższy do zmiany po tym, jak powstaną `pathon-scene`, `pathon-skin` i paczki prepassu, więc zapada teraz.

## Decyzja

### Pula: mega-bufory w chunkach, alokacja w jednostkach wierzchołków i indeksów

- Jedna `GeometryPool` obsługuje **jeden layout wierzchołków**. Meshe o innym layoucie idą do osobnej puli.
- Pamięć to **chunki**: każdy chunk ma po jednym buforze na strumień wierzchołków i osobny bufor indeksów. Chunk ma stały rozmiar (`chunk_vertices` / `chunk_indices`); mesh większy niż chunk dostaje chunk własny, dokładnie tak duży, jak trzeba, do twardego limitu pojedynczego bufora.
- Wierzchołki i indeksy mają **osobne alokatory** (`RangeAllocator` z `pathon-resources`), a pozycja mesha to numer chunka i zakres w **jednostkach** (wierzchołkach, indeksach), nie w bajtach. Wszystkie strumienie jednego chunka mają tę samą pojemność w wierzchołkach, więc ten sam `base_vertex` adresuje każdy strumień. To warunek `draw_indexed_indirect` i multi-draw (M4-11), gdzie `base_vertex` siedzi w argumentach, a bindy są stałe.
- **Indeksy są zawsze `u32`.** `u16` z wejścia jest rozszerzany przy uploadzie. Cena to 2× pamięci indeksów małych meshy; zysk to jeden format w całej puli (jeden `set_index_buffer` dla multi-draw) i indeksy czytelne jako `array<u32>` w vertex pull i RT bez rozpakowywania. Indeksy w puli są **względne** wobec mesha (od 0); shader dodaje `base_vertex`.
- Twardy limit chunka: `min(max_buffer_size, max_storage_buffer_binding_size)` urządzenia, bo chunk bywa bindowany w całości jako storage (WebGPU: 128 MiB), oraz `i32::MAX` wierzchołków (`base_vertex` jest `i32`). Budżet puli (`max_vertices`, `max_indices`) jest sumą po chunkach.

### Flagi użycia buforów

- Chunk wierzchołków: `VERTEX | STORAGE | COPY_DST | COPY_SRC`; chunk indeksów: `INDEX | STORAGE | COPY_DST | COPY_SRC`. `STORAGE` daje vertex pull (visibility buffer, skinning, RT) bez drugiej kopii; `COPY_SRC` to przyszła defragmentacja i odczyt w testach. Bufor wejściowy BLAS (`BLAS_INPUT`) dojdzie z M8-03, gdy `BufferUsage` go dostanie.
- Dane wchodzą przez `DeviceApi::write_buffer` (ADR-0020: zapis uporządkowany przed następnym `submit`). Staging ring nie jest potrzebny: `write_buffer` w wgpu stage'uje sam, a w trybie A kolejność zapewnia host.

### Fragmentacja

- **Sub-alokacja first-fit z scalaniem** (`RangeAllocator`) w obrębie chunka, chunki szukane po kolei. First-fit od najniższego adresu trzyma wolne miejsce na końcu chunka; best-fit i bucketowy alokator (offset-allocator) są ścieżką ulepszenia po pomiarze (M4-17), za tym samym interfejsem.
- **Zwolnienie jest odroczone o `frames_in_flight`** (ADR-0022): lista nagrana, a jeszcze niezgłoszona (tryb A), może czytać zakres, który właśnie oddano. Uchwyt mesha przestaje działać od razu, zakres wraca do alokatora dopiero w `begin_frame`.
- **Pusty chunk jest zwalniany** (jego bufory niszczone od razu, bo ostatni zakres wrócił dopiero po `frames_in_flight`), o ile nie jest jedynym chunkiem standardowego rozmiaru; chunk dedykowany dla dużego mesha znika zawsze. Dzięki temu streaming nie zostawia pamięci po szczycie.
- **Adresowanie jest relokowalne**: nikt poza pulą nie trzyma offsetów, tylko `MeshId`; lokalizację (`MeshLocation`) czyta się co klatkę. Defragmentacja (kopie `copy_buffer_to_buffer` w liście komend i podmiana lokalizacji) mieści się więc w tym modelu bez zmiany API; nie jest częścią M4-01.
- **Diagnostyka rozróżnia przyczyny:** `M0003` budżet puli wyczerpany (wolne miejsce naprawdę zabrakło), `M0004` jest łączne wolne miejsce, ale brak ciągłego zakresu (fragmentacja albo zwolnienia czekają na koniec klatek w locie), `M0005` mesh większy niż może pomieścić jeden bufor. `PoolStats` podaje `largest_free` i `fragmentation` per strumień, żeby paczka ostrzegała, zanim dojdzie do błędu.

### Layouty wierzchołków

- `VertexLayout` to opis w danych, bez zależności od wgpu: lista **strumieni**, każdy ze `stride` i atrybutami (`Semantic`, `VertexFormat`, offset). Semantyki mają **stałe lokacje shaderowe** (Position 0, Normal 1, Tangent 2, Uv0 3, Uv1 4, Color 5, Joints 6, Weights 7), więc shader nie zależy od układu strumieni. `vertex_buffer_layouts()` mapuje layout na `Vec<VertexBufferLayout>` z `pathon-rhi-types` do `RenderPipelineDesc`; `VertexLayout` jest `Eq + Hash`, więc wchodzi do klucza potoku (ADR-0023).
- `Position` (`Float32x3`) jest obowiązkowa: z niej liczą się granice (AABB), culling i BLAS.
- Layout standardowy: strumień 0 to **same pozycje** (12 B; prepass głębi i cienie czytają tylko ten strumień), strumień 1 to powierzchnia (`Normal` `Float32x3` @0, `Tangent` `Float32x4` @12, `Uv0` `Float32x2` @28, stride 36). Skinning dokłada strumień 2 (patrz niżej). Formaty są nieskompresowane (f32) celowo: kompresja (oct-normals, f16 uv) to inny layout, bez zmiany API, gdy pomiar jej zażąda (M4-17).

### Dane poprzedniej klatki

- **Transformacje obiektów** (`TransformHistory`): jeden storage buffer par `ObjectTransforms { model, previous_model }` (2 × afiniczne 3×4, po 48 B, razem 96 B, wiersze jako `vec4`; w WGSL `array<ObjectTransforms>` ze `struct { model: mat3x4<f32>, previous_model: mat3x4<f32> }`, pozycja `v4 * m` daje `vec3`). Para jest przeplatana, bo vertex shader czyta obie w jednym dostępie. `begin_frame(frame)` kopiuje `model` do `previous_model` **tylko dla obiektów ruszonych w poprzedniej klatce** (O(ruszone), nie O(wszystkie)); nowy obiekt ma `previous_model == model` (zero ruchu przy pierwszym pojawieniu). `begin_frame` jest idempotentne dla tego samego numeru klatki.
- **Pozycje po skinningu** (`DeformedPositions`, układ dla M4-12): osobny storage buffer `array<vec4<f32>>` (xyz = pozycja w przestrzeni obiektu, w = 1; 16 B na wierzchołek) z **dwoma bankami** o równej pojemności. Instancja skinowana dostaje raz zakres `DeformedRange { first_vertex, vertex_count }`, ważny w obu bankach. W klatce `f` compute zapisuje bank `f & 1`, a wektory ruchu czytają poprzednią pozycję z banku `(f & 1) ^ 1`; adres wierzchołka to `(bank * bank_capacity + first_vertex + vertex_id)`. Bufor ma też `VERTEX`, więc pass rasteryzacji może związać bank jako strumień pozycji (`Float32x4`, stride 16) w miejsce strumienia 0 mesha. Normalne i tangenty po skinningu piszą się do osobnego bufora jednobankowego o układzie strumienia 1 (poprzednie normalne nie są potrzebne wektorom ruchu).
- **Strumień skinningu** (strumień 2 layoutu skinowanego): `Joints` `Uint32x2` (cztery indeksy `u16` spakowane po dwa w `u32`, młodszy pierwszy) @0, `Weights` `Unorm8x4` @8, stride 12. Morph targets dołoży M4-12 jako osobny bufor delt, bez zmiany tego layoutu.
- Klatki i `frames_in_flight` podaje wołający (`ViewCtx::frame`, ADR-0022); pula i historia nie czytają zegara i nie tworzą wątków.

### Granica crate'u i polityka rozrostu

- `pathon-mesh` jest zwykłą paczką (`[package.metadata.pathon] kind = "pack"`): zależy tylko od `pathon-api`, nie wchodzi do domknięcia rdzenia i nie dokłada crate'ów do jego bazowej liczby.
- Nowe publiczne struktury mają `#[non_exhaustive]` i konstruktory (ADR-0020): `VertexLayout` (pola prywatne, walidowany `new`), `PoolDesc`, `MeshData`, `MeshLocation`, `PoolStats`, `MeshInfo`.
- Diagnostyki mają prefiks **M** (mesh): `M0001`…`M0006`, strony w `docs/diag/`.

## Konsekwencje

- Jedno `base_vertex` na mesh w całej puli: layouty z różnym `stride` w strumieniu pozycji i powierzchni nie mają osobnych adresowań, a multi-draw (M4-11) nie wymaga przeliczania offsetów.
- Pamięć indeksów małych meshy jest 2× większa niż w `u16` — do ponownej oceny z pomiarem (M4-17).
- Uchwyt `MeshId` nie niesie identyfikatora puli: użycie w innej puli jest błędem wołającego (id z innej puli może trafić w cudzy slot).
- Pula nie trzyma kopii CPU geometrii. Po utracie urządzenia `forget_gpu()` unieważnia wszystkie uchwyty, a aplikacja wgrywa meshe ponownie (callback z M1-03).
- Zmiana struktury `ObjectTransforms` albo układu banków skinningu po M4-12 wymaga nowego ADR: dotyka shaderów materiałów, prepassu i skinningu naraz.
