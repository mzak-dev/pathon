# pathon-pbr

Oświetlenie fizyczne sceny (M4-05, [ADR-0042](../adr/0042-pbr-prepass-klastry-i-forward.md)): prepass (`depth`, `normals`, `motion`, `material_id`, na żądanie `roughness`, `albedo` i `depth_pyramid`), przypisanie świateł do klastrów w compute i pass koloru, który woła `material::standard::evaluate` i `ibl::shading::ibl_evaluate` i pisze `hdr` (oraz `diffuse` i `specular` na żądanie).

To zwykła paczka (`[package.metadata.pathon] kind = "pack"`): zależy od `pathon-api`, `pathon-shader` i od crate'ów `pack-lib` ([ADR-0041](../adr/0041-pack-lib.md)): `pathon-scene`, `pathon-mesh`, `pathon-material`, `pathon-bindless`, `pathon-ibl`, `pathon-shadow`. Nie zna ECS, plików ani okna: scenę, geometrię i materiały dostaje przez `Shared`. Cieni nie rysuje (robi to `ShadowPack`, który tylko czyta), nie robi też nieba, cullingu, skinningu ani TAA.

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
| `ShadowFrame` i slot `shadow.map` | nie | publikuje `ShadowPack` ([pathon-shadow](pathon-shadow.md)); gdy są oba, pass koloru używa permutacji z flagą `shadows` |

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

Siatka kafli (`tile_size` px, potęga dwójki) razy wykładnicze warstwy głębi (`slices` od `near` do `far`). Compute (`cluster.wesl`): jeden wątek na klaster liczy jego AABB w przestrzeni widoku (perspektywa i ortografia), zlicza światła, których sfera zasięgu go dotyka, rezerwuje miejsce na liście jednym `atomicAdd` na `u32` i zapisuje indeksy w kolejności świateł. Nie ma atomików 64-bit, więc ten sam shader działa na Web. Światła kierunkowe nie są klastrowane. Zasięg punktowego i reflektora to `Light::influence_radius`, a tłumienie ma okno `saturate(1 − (d/r)⁴)²` (Karis 2013), więc światło znika dokładnie na zasięgu i klaster nic nie zmienia w obrazie. Lista jest tak duża, jak trzeba: `min(max_light_indices, klastry × światła)` wpisów (domyślny limit 2²⁴), więc przepełnienie (`L0005`) wymaga jawnie mniejszego limitu; wtedy skraca listy losowych klastrów, bo kolejność rezerwacji zależy od planowania wątków.

`LightAssignment::BruteForce` testuje w kolorze każde światło i służy do sprawdzania klastrów.

## Pass koloru

Forward z prepassem: kolor testuje głębię `GreaterEqual` bez zapisu, więc każdy piksel jest cieniowany raz. Masked odrzuca fragmenty już w prepassie. Dla każdego światła: `LightSample` (kierunek, promieniowanie = kolor · lux albo kandela · tłumienie), `shadow_factor` (hak aplikacji, domyślnie 1) razy mapa kaskad, `evaluate` osobno na `diffuse` i `specular`. Środowisko: `ibl_evaluate`; okluzja materiału zaciemnia dyfuzję, specular dostaje okluzję Lagarde'a (2014), oba korektę wielokrotnego odbicia Jimenez i in. (2016). Obiekty z `alpha: Blend` idą osobnym passem po opaque, posortowane od najdalszych, premultiplied, z testem głębi bez zapisu.

Jednostki: lux (kierunkowe), kandela (punktowe i reflektory, lumeny / 4π jak w glTF). `hdr` jest bez ekspozycji; jasność wyświetlana to `hdr × exposure` ([pathon-post](pathon-post.md)).

## Światła powierzchniowe

`LightKind::Area` (prostokąt, moc w lumenach, luminancja L = P/(π·A); [ADR-0048](../adr/0048-swiatla-powierzchniowe-prostokatne.md)) idzie tą samą tablicą świateł i tymi samymi klastrami co punktowe: `GpuLight` (96 bajtów) niesie środek, połówki boków i `kolor · L`, a promień klastra to `influence_radius` (zasięg wyprowadzony z natężenia osiowego P/π plus połowa przekątnej, więc wielki prostokąt nie wypada z klastrów). Gdy w scenie jest choć jedno takie światło, pass koloru buduje permutację z flagą WESL `area_lights` (`ForwardKey::area`): dochodzi binding `LTC_BINDING` (13) z tablicą 32×32 węzłów jako bufor storage i moduł `pbr::area`.

- **Diffuse:** analityczna całka Lamberta z prostokąta (wielokąt sferyczny obcięty do horyzontu normalnej, wzór Arvo 1995), razy `front_diffuse` i L; błąd względem całkowania numerycznego 0,1%.
- **Specular:** LTC dla GGX (Heitz i in. 2016) z **własną tablicą** (`src/ltc_table.rs`, generuje `tools/pathon-ltc-fit`: `cargo run -p pathon-ltc-fit --release`, 30 s); w tablicy też całki lobu dla `f0` i `f90`, więc działa bez `IblPack`. Clear coat jako drugi lob.
- **Jednostki:** prostokąt o mocy P świeci jak powierzchnia Lamberta: na osi P/π cd, daleko zbliża się do źródła punktowego o natężeniu (P/π)·cos θ (błąd < 2% w odległości 10× rozmiar).
- **Granice:** bez cieni, bez tekstury, dysk jako prostokąt o tym samym polu; jakość specular przy widoku stycznym (n·v < 0,3) i chropowatości 0,3 do 0,6 jest ograniczona dopasowaniem (wariacja całkowita lobu do 0,16, w teście na GPU do 53% jasności piksela dla wąskiego odblasku), przy widoku bliższym normalnej średnio 1 do 3%.
- Moduł `area` (CPU) to model tych całek (`cosine_integral`, `diffuse_cosine_integral`, `specular_ltc`) i ich referencje numeryczne (`diffuse_cosine_integral_numeric`, `specular_numeric`); `ltc` to tablica i jej wyszukiwanie, lustrzane do shadera.

## Cienie

Cieniowane światło to jedno światło kierunkowe, które `ShadowPack` rysuje do `shadow.map`; reszta świateł (punktowe, reflektory) cieni nie ma. Gdy w klatce są `ShadowFrame` i slot `shadow.map`, `LightingFeature` buduje permutację z flagą WESL `shadows` (`ForwardKey::shadows`): w grupie 0 dochodzą trzy bindingi `SHADOW_FIRST_BINDING`… (kaskady, tablica głębi, sampler porównawczy, tuż za bindingami IBL), a `add_light` mnoży promieniowanie przez `shadow_factor_directional` z `package::shadow::sample`. Normalną geometryczną (po odwróceniu dla tylnej ściany), głębię widoku i `@builtin(position).xy` shader trzyma w zmiennej prywatnej, wypełnianej raz w `shade`. Bez światła w `ShadowScene` paczka cieni nie publikuje niczego i działa permutacja bez cieni (układ bindingów jak dawniej).

`light_index` w `DirectionalShadowLight` to indeks w tablicy świateł paczki pbr, w której światła kierunkowe są pierwsze (`PackedLights`): słońce będące jedynym światłem kierunkowym ma indeks 0, niezależnie od kolejności w scenie. Hak `pbr::hooks::shadow_factor(light_index, position)` zostaje dla własnych cieni aplikacji i mnoży się z mapą.

## Ograniczenia

- Brak cullingu: rysowane są wszystkie widoczne instancje, jedno wywołanie na instancję (M4-11).
- `CullMode::None`: podwójna strona rozstrzyga się przez `front_facing`; transformacje lustrzane odwracają znak stycznej niepoprawnie.
- Wgrywanie instancji i świateł co klatkę w całości (przyrostowe z `changes()` zostaje na M4-11).
- Układy grupy 0 są reflektowane raz na permutację (cache w `PbrShaders`); wcześniej parsowanie WGSL szło co klatkę (14 ms CPU na scenie z 862 instancjami).
- Koszt przypisania to `O(klastry × światła)`; hierarchiczne przycinanie do zmierzenia w M4-17.
- Światła powierzchniowe bez cieni; brak AA specular, okluzji z bent normals, cieni punktowych i reflektorów, nieba, przezroczystości z transmisją (M4-15) i MSAA (M4-19).

## Diagnostyka

| Kod | Sytuacja |
| --- | --- |
| [L0001](../diag/L0001.md) | niepoprawne `PbrSettings` (zastąpione domyślnymi) |
| [L0002](../diag/L0002.md) | układ wierzchołków puli bez normalnych, stycznych albo uv |
| [L0003](../diag/L0003.md) | instancje pominięte: nieznany mesh albo materiał |
| [L0004](../diag/L0004.md) | brak wymaganych danych w `Shared` |
| [L0005](../diag/L0005.md) | lista indeksów klastrów przepełniona |
| [L0006](../diag/L0006.md) | urządzenie nie obsługuje formatów PBR |
| [E0001](../diag/E0001.md) | jawny zasięg światła powierzchniowego krótszy niż połowa jego przekątnej |

## Testy

- `src` (bez GPU): jednostkowe (układ `GpuLight` i `GpuInstance`, macierz normalnych, siatka i warstwy klastrów, jitter i historia kamery, bufor roboczy, referencje CPU).
- `tests/shaders.rs` (`rhi-null`): wszystkie korzenie składają się i przechodzą naga, potoki prepassu i koloru istnieją dla 48 permutacji materiału na obu ścieżkach tekstur (z `binding_array` i bez), z binderem i bez, potoki klastrów i piramidy.
- `tests/clusters.rs` (`--features gpu-tests`): listy z GPU kontra brute force w `f64` na losowych scenach (10, 100, 1000 świateł; perspektywa i ortografia), punkty losowe dostają każde światło, które je dosięga, przepełnienie jest liczone i nie wychodzi poza listę, zero świateł; na `rhi-null` walidacja komend.
- `tests/prepass.rs`: głębia (reverse-Z), normalne, id materiałów, roughness i albedo względem geometrii sceny; ruch obiektu i ruch kamery w pikselach (±0,5); piramida względem referencji CPU, bit w bit.
- `tests/golden_scene.rs`: pełny łańcuch (`IblPack`, `PbrPack`, `PostPack`) na scenie testowej (podłoga, kule metal i dielektryk, sześcian, ogrodzenie masked, cztery materiały, trzy z teksturami; słońce, trzy światła punktowe i reflektor): golden `tests/golden/pbr_scene.png`, brak NaN, `diffuse + specular = hdr`, klastry równe `BruteForce` z tysiącem świateł, ścieżka per-materiał równa bindless, przezroczysta tafla; na `rhi-null` cała scena ze wszystkimi slotami, także z `ShadowPack` (z cieniem i bez światła do cieniowania); na GPU cień słońca przyciemnia podłogę za sześcianem i niczego nie rozjaśnia. `tests/shaders.rs`: permutacja `shadows` dodaje dokładnie trzy bindingi, permutacja `area_lights` jeden (`LTC_BINDING`).
- `tests/area_lights.rs`: światło daleko kontra źródło punktowe (< 2%), diffuse kontra kwadratura, specular LTC kontra źródło punktowe i (GPU) kontra całkowanie numeryczne GGX po prostokącie na podłodze (3 chropowatości, 2 światła), dwustronność z podwójną mocą, światło za powierzchnią i odwrócone, prostokąty przecinające powierzchnię bez NaN, 100 świateł w klastrach równe `BruteForce`, golden `tests/golden/pbr_area_light.png`; `tests/clusters.rs`: 100 świateł powierzchniowych kontra brute force CPU.

```powershell
cargo test -p pathon-pbr                                                    # bez GPU
$env:PATHON_TEST_BACKEND="vulkan"; cargo test -p pathon-pbr --features gpu-tests -- --test-threads=1
cargo run -p pathon-examples --bin pbr_scene -- pbr_scene.png
```
