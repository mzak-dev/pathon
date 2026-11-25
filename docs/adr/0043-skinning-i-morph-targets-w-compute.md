# ADR-0043: Skinning i morph targets w compute

**Status:** Przyjęta
**Data:** 2026-11-25

## Kontekst

Wektory ruchu dla TAA (M5-01) potrzebują pozycji wierzchołka z poprzedniej klatki, także dla meshy deformowanych na GPU. ADR-0027 ustalił układ (`DeformedPositions`: dwa banki `vec4<f32>`, normalne i tangenty w osobnym buforze jednobankowym, strumień `Joints`/`Weights`); M4-12 dostarcza paczkę, która go zapisuje. Web ma compute, więc jest jedna ścieżka (bez wariantu vertex shadera).

## Decyzja

### Praca i kolejność

- `SkinPack` (`kind = "pack"`: `pathon-api`, `pathon-mesh`, dev: `pathon-shader`) to `RenderFeature` w `Stage::Setup`. Jego pass compute (`skin`, priorytet −10, `side_effect`) wykonuje się przed `Shadows` i `Prepass`, które czytają pozycje po deformacji; priorytet −10 wyprzedza „clear output” z rdzenia w tym samym etapie bez remisu (G0006). Nie ma roli (`Role`): nikt nie zastępuje skinningu inną paczką.
- Praca idzie **raz na próbę nagrania klatki** (`ViewCtx::record_epoch`), niezależnie od liczby widoków; widoki dalsze tylko importują bufory i publikują uchwyty. Skinning jest idempotentny (pisze bank `frame & 1`), więc powtórzona klatka (ADR-0022) daje ten sam wynik.
- Bufory wynikowe należą do aplikacji (alokuje `DeformedRange` z `DeformedPositions`), pack dostaje je w `SkinFrame` w `Shared`; po klatce pack publikuje `SkinOutput` z uchwytami `BufferHandle` (importowane bufory po passie compute). Pass, który czyta wynik (prepass, cienie), deklaruje `read_buffer(.., Access::VERTEX)` i graf sam układa kolejność i barierę `STORAGE_WRITE → VERTEX` (ADR-0011).

### Obliczenia

- **Jeden dispatch na instancję** (64 wątki na wierzchołek, 2D przy więcej niż `max_compute_workgroups_per_dimension` grup). Parametry instancji siedzą w jednym buforze uniform, wiązane zakresem (`offset`) z krokiem wyrównanym do `min_uniform_buffer_offset_alignment`: przeglądarki nie mają immediates (ADR-0018), a dynamicznych offsetów RHI nie ma. Jedna grupa wiązań na instancję, tworzona w passie.
- Pozycje wejściowe, powierzchnia i strumień skinningu są czytane **wprost z buforów chunka puli** (vertex pull, ADR-0027): bez kopii. Osiem bindingów (uniform + 7 storage) mieści się w limicie 8 buforów storage na etap, który gwarantuje WebGPU.
- **Kolejność:** morph targets na pozie spoczynkowej (`p += Σ wₖ·Δpₖ`, tak samo normalne i tangenty), potem macierz kości, na końcu normalizacja normalnej i tangenty (długość² < 1e−20 zostawia wektor bez zmian: bez NaN). Delty z wagą dokładnie 0 są pomijane.
- **Macierz kości:** `Σ wᵢ·Mᵢ / Σ wᵢ` po wpływach z wagą > 0 i indeksem < liczba kości instancji. Renormalizacja naprawia sumę Unorm8 różną od 1 (np. 254/255), indeks poza paletą jest pomijany, a wierzchołek bez żadnego wpływu dostaje macierz jednostkową. Normalne i tangenty używają górnej macierzy 3×3 (jak glTF), więc nierównomierna skala w kościach nie jest korygowana (odwrotna transpozycja nie jest liczona).
- **Macierze kości** to gotowe macierze skinningu w przestrzeni obiektu (poza stawu pomnożona przez odwrotną macierz bindu), kolumnami jak `Mat4`; `SkinFrame` ma jeden wektor macierzy i jeden wektor wag morphów, a instancja dostaje plasterki. Animację (próbkowanie klipów, hierarchia) liczy aplikacja albo host.
- **Morph targets gęste** (`[cel][wierzchołek][pozycja, normalna, tangent]`, 48 B na parę): proste i bez wyszukiwania per wierzchołek. Sparse (listy `(indeks, delta)`) oszczędzałoby pamięć dla lokalnych kształtów (twarz), ale wymaga wyszukiwania lub osobnego passu; wejdzie za tym samym API po pomiarze (M4-17).

### Dane poprzedniej klatki

- W klatce `f` pass pisze bank `f & 1`; bank drugi trzyma pozę poprzedniej klatki, o ile instancja była deformowana w klatce `f − 1`. Aplikacja zgłasza to w `SkinInstance::history`: `Fresh` (domyślnie; pierwsza klatka, po pominięciu klatki albo cięciu) zapisuje bieżącą pozę **do obu banków**, więc ruch wynosi zero; `Continuing` zostawia drugi bank.
- Normalne i tangenty piszą się do `DeformedSurface`: jeden bank o układzie strumienia 1 `standard()` (36 B: `Normal`, `Tangent`, `Uv0`; UV kopiowane, żeby bufor mógł zastąpić strumień 1 w passie rasteryzacji). Poprzednie normalne nie są potrzebne wektorom ruchu.
- Pozycje są w przestrzeni obiektu; `previous_model` (`TransformHistory`) dokłada ruch całego obiektu po stronie shadera ruchu.

### Walidacja i diagnostyki

- Instancja, która nie pasuje do buforów, jest pomijana z `W0001` (reszta klatki działa); komunikaty są zgłaszane przy zmianie, nie co klatkę. `W0002`: dane morphów; `W0003`: bufory wynikowe. Brak pracy usuwa `SkinOutput` z `Shared`.
- `negotiate` wyłącza paczkę (`F0101`), gdy urządzenie nie ma grup 64 wątków.

## Granica „Gotowe gdy”

Plan wymaga „animowanego modelu z poprawnym TAA (bez smużenia) w M5”. TAA powstaje w M5-01, więc M4-12 dowodzi **kontraktu**: test GPU (Vulkan, RX 9070 XT) sprawdza, że `current` zgadza się z referencją CPU (±1e-4), `previous` to poza poprzedniej klatki, a ich różnica to wektor ruchu z wzoru analitycznego. Końcowy test z TAA (animowany model bez smużenia, w tym podpięcie pozycji poprzedniej klatki w prepassie `pathon-pbr`) zostaje w M5-01.

## Konsekwencje

- Prepass i cienie paczki `pathon-pbr` czytają dziś pozycje z puli; podpięcie ich do `SkinOutput` (strumień 0 ← bank, strumień 1 ← `DeformedSurface`, `deformed_vertex_buffers()`) to osobna zmiana po stronie `pathon-pbr` / `pathon-shadow`.
- Jedna paczka ma 8 bindingów; dodanie kolejnego strumienia (np. drugi zestaw wag, kolory) wymaga złożenia bufora albo drugiego passu.
- Cena jednego dispatchu na instancję to narzut przy tysiącach małych instancji; łączenie w jeden dispatch po grupach źródeł jest ścieżką ulepszenia po pomiarze (M4-17).
- Nowe publiczne struktury mają `#[non_exhaustive]` i konstruktory (ADR-0020).
