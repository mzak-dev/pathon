# pathon-pbr

Oświetlenie fizyczne sceny (M4-05, [ADR-0042](../adr/0042-pbr-prepass-klastry-i-forward.md)): prepass (`depth`, `normals`, `motion`, `material_id`, na żądanie `roughness`, `albedo` i `depth_pyramid`), przypisanie świateł do klastrów w compute i pass koloru, który woła `material::standard::evaluate` i `ibl::shading::ibl_evaluate` i pisze `hdr` (oraz `diffuse` i `specular` na żądanie).

To zwykła paczka (`[package.metadata.pathon] kind = "pack"`): zależy od `pathon-api`, `pathon-shader` i od crate'ów `pack-lib` ([ADR-0041](../adr/0041-pack-lib.md)): `pathon-scene`, `pathon-mesh`, `pathon-material`, `pathon-bindless`, `pathon-ibl`. Nie zna ECS, plików ani okna: scenę, geometrię i materiały dostaje przez `Shared`. Nie robi cieni, nieba, cullingu, skinningu ani TAA.

```rust,ignore
let mut renderer = Renderer::builder()
    .with_pack(IblPack::new())
    .with_shared(IblSource::new(Arc::new(sky), IblSettings::DESKTOP))
    .with_pack(PbrPack::new())
    .with_pack(PostPack::new())
    .build()?;
let target = renderer.add_offscreen_target(PhysicalSize::new(1280, 720), Format::Rgba8Unorm)?;

// Dane hosta: po zbudowaniu renderera, bo potrzebują urządzenia.
let shared = renderer.shared_mut();
shared.insert(pool);       // pathon_mesh::GeometryPool z VertexLayout::standard()
shared.insert(materials);  // pathon_material::MaterialTable
shared.insert(binder);     // opcjonalnie: pathon_bindless::TextureBinder
// co klatkę:
renderer.shared_mut().insert(scene.snapshot());   // pathon_scene::Snapshot
renderer.render_frame(&FrameInput { views: vec![ViewInput { target, camera }], ..FrameInput::default() })?;
```

Przykład: `cargo run -p pathon-examples --bin pbr_scene -- wynik.png` ([przykłady](../examples.md)).

## Wejścia (`Shared`)

| Typ | Wymagany | Uwagi |
| --- | --- | --- |
| `pathon_scene::Snapshot` | tak | co klatkę; instancje z flagą `VISIBLE`, światła z `lights()` |
| `GeometryPool` | tak | układ z atrybutami `Normal`, `Tangent`, `Uv0` (`standard()` albo `skinned()`); inaczej `L0002` |
| `MaterialTable` | tak | paczka sama wysyła zmiany (`sync`); `MaterialHandle::DEFAULT` to materiał o indeksie 0 |
| `TextureBinder` | nie | wybór ścieżki tekstur z `Caps` ([pathon-bindless](pathon-bindless.md)); paczka sama woła `prepare` i buduje grupy materiałów; bez niego materiały są bez tekstur |
| `MaterialHooks` | nie | hook powierzchni aplikacji; `texture_source` ustawia paczka z binderu |
| `IblEnvironment` | nie | publikuje `IblPack`; bez niego środowisko jest czarne |
| `PbrSettings` | nie | `ClusterSettings`, `LightAssignment`, `environment_intensity`; zmieniane między klatkami |

Brak któregokolwiek z wymaganych daje `L0004` (raz) i puste sloty. Kamera to `ViewInput::camera` **bez jittera**: paczka nakłada `ViewCtx::jitter` sama i trzyma poprzednią macierz widoku per `ViewId`.

## Sloty

| Slot | Kiedy | Zawartość |
| --- | --- | --- |
| `depth` | zawsze | reverse-Z, 0 tam, gdzie nic nie narysowano |
| `normals` | zawsze | normalna shadingu w świecie (z mapą normalnych), oktaedralnie w `Rg16Float` |
| `motion` | zawsze | piksele: `poprzedni − bieżący`, y w dół, bez jittera |
| `material_id` | zawsze | indeks w `MaterialTable`; `u32::MAX` na tle |
| `roughness`, `albedo` | `request` | chropowatość percepcyjna; albedo (kolor bazowy) w RGB i `metallic` w alfa |
| `depth_pyramid` | `request` | `R32Float`, minimum (najdalsza głębia) bloku; mipy do 1×1; nieparzysty rozmiar źródła: ostatni teksel obejmuje trzy wiersze albo kolumny |
| `hdr` | zawsze | liniowe, bez ekspozycji |
| `diffuse`, `specular` | `request` | `diffuse + specular = hdr`; emisja jest w `specular` |

Dekodowanie normalnych i referencje CPU (`oct_encode`, `oct_decode`, `depth_pyramid`, `motion_of`, `assign_lights`) są w module `reference` (`f64` tam, gdzie liczy się precyzja).

## Klastry świateł

Siatka kafli (`tile_size` px, potęga dwójki) razy wykładnicze warstwy głębi (`slices` od `near` do `far`). Compute (`cluster.wesl`): jeden wątek na klaster liczy jego AABB w przestrzeni widoku (perspektywa i ortografia), zlicza światła, których sfera zasięgu go dotyka, rezerwuje miejsce na liście jednym `atomicAdd` na `u32` i zapisuje indeksy w kolejności świateł. Nie ma atomików 64-bit, więc ten sam shader działa na Web. Światła kierunkowe nie są klastrowane. Zasięg punktowego i reflektora to `Light::influence_radius`, a tłumienie ma okno `saturate(1 − (d/r)⁴)²` (Karis 2013), więc światło znika dokładnie na zasięgu i klaster nic nie zmienia w obrazie. Przepełnienie wspólnej listy (`max_light_indices`) skraca listy późniejszych klastrów (`L0005`).

`LightAssignment::BruteForce` testuje w kolorze każde światło i służy do sprawdzania klastrów.

## Pass koloru

Forward z prepassem: kolor testuje głębię `GreaterEqual` bez zapisu, więc każdy piksel jest cieniowany raz. Masked odrzuca fragmenty już w prepassie. Dla każdego światła: `LightSample` (kierunek, promieniowanie = kolor · lux albo kandela · tłumienie), `shadow_factor` (hak, domyślnie 1; podmienia go `pathon-shadow`), `evaluate` osobno na `diffuse` i `specular`. Środowisko: `ibl_evaluate`; okluzja materiału zaciemnia dyfuzję, specular dostaje okluzję Lagarde'a (2014), oba korektę wielokrotnego odbicia Jimenez i in. (2016). Obiekty z `alpha: Blend` idą osobnym passem po opaque, posortowane od najdalszych, premultiplied, z testem głębi bez zapisu.

Jednostki: lux (kierunkowe), kandela (punktowe i reflektory, lumeny / 4π jak w glTF). `hdr` jest bez ekspozycji; jasność wyświetlana to `hdr × exposure` ([pathon-post](pathon-post.md)).

## Ograniczenia

- Brak cullingu: rysowane są wszystkie widoczne instancje, jedno wywołanie na instancję (M4-11).
- `CullMode::None`: podwójna strona rozstrzyga się przez `front_facing`; transformacje lustrzane odwracają znak stycznej niepoprawnie.
- Wgrywanie instancji i świateł co klatkę w całości (przyrostowe z `changes()` zostaje na M4-11).
- Koszt przypisania to `O(klastry × światła)`; hierarchiczne przycinanie do zmierzenia w M4-17.
- Brak AA specular, okluzji z bent normals, cieni, nieba, przezroczystości z transmisją (M4-15) i MSAA (M4-19).

## Diagnostyka

| Kod | Sytuacja |
| --- | --- |
| [L0001](../diag/L0001.md) | niepoprawne `PbrSettings` (zastąpione domyślnymi) |
| [L0002](../diag/L0002.md) | układ wierzchołków puli bez normalnych, stycznych albo uv |
| [L0003](../diag/L0003.md) | instancje pominięte: nieznany mesh albo materiał |
| [L0004](../diag/L0004.md) | brak wymaganych danych w `Shared` |
| [L0005](../diag/L0005.md) | lista indeksów klastrów przepełniona |
| [L0006](../diag/L0006.md) | urządzenie nie obsługuje formatów PBR |

## Testy

- `src` (bez GPU): jednostkowe (układ `GpuLight` i `GpuInstance`, macierz normalnych, siatka i warstwy klastrów, jitter i historia kamery, bufor roboczy, referencje CPU).
- `tests/shaders.rs` (`rhi-null`): wszystkie korzenie składają się i przechodzą naga, potoki prepassu i koloru istnieją dla 48 permutacji materiału na obu ścieżkach tekstur (z `binding_array` i bez), z binderem i bez, potoki klastrów i piramidy.
- `tests/clusters.rs` (`--features gpu-tests`): listy z GPU kontra brute force w `f64` na losowych scenach (10, 100, 1000 świateł; perspektywa i ortografia), punkty losowe dostają każde światło, które je dosięga, przepełnienie jest liczone i nie wychodzi poza listę, zero świateł; na `rhi-null` walidacja komend.
- `tests/prepass.rs`: głębia (reverse-Z), normalne, id materiałów, roughness i albedo względem geometrii sceny; ruch obiektu i ruch kamery w pikselach (±0,5); piramida względem referencji CPU, bit w bit.
- `tests/golden_scene.rs`: pełny łańcuch (`IblPack`, `PbrPack`, `PostPack`) na scenie testowej (podłoga, kule metal i dielektryk, sześcian, ogrodzenie masked, cztery materiały, trzy z teksturami; słońce, trzy światła punktowe i reflektor): golden `tests/golden/pbr_scene.png`, brak NaN, `diffuse + specular = hdr`, klastry równe `BruteForce` z tysiącem świateł, ścieżka per-materiał równa bindless, przezroczysta tafla; na `rhi-null` cała scena ze wszystkimi slotami.

```powershell
cargo test -p pathon-pbr                                                    # bez GPU
$env:PATHON_TEST_BACKEND="vulkan"; cargo test -p pathon-pbr --features gpu-tests -- --test-threads=1
cargo run -p pathon-examples --bin pbr_scene -- pbr_scene.png
```
