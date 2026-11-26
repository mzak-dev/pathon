# pathon-shadow

Cienie światła kierunkowego z map kaskadowych (M4-07, [ADR-0044](../adr/0044-cienie-kaskadowe-csm.md)): od 1 do 4 kaskad podzielonych wzdłuż głębi kamery, każda w warstwie jednej tekstury `Depth32Float` (reverse-Z), filtr PCF, stabilizacja do tekseli i trzy warstwy biasu. Do tego cienie świateł punktowych i reflektorów (K03, [ADR-0050](../adr/0050-cienie-swiatel-punktowych-i-reflektorow.md)): mapy perspektywiczne w jednym atlasie, sześć ścian na światło punktowe, wybór najważniejszych świateł z histerezą, rozdzielczość zależna od odległości i pamięć podręczna map (mapa światła, które się nie zmieniło, nie jest rysowana ponownie). Pass cieni jest węzłem grafu w `Stage::Shadows` i trzyma rolę `Role::SHADOWS`; shader oświetlenia czyta wynik funkcjami WGSL `shadow_factor_directional` i `local_shadow_factor`.

To `pack-lib` (`[package.metadata.pathon] kind = "pack-lib"`, [ADR-0041](../adr/0041-pack-lib.md)): `PbrPack` zależy od niego, żeby czytać `ShadowFrame`, slot i moduł WGSL, a `ShadowPack` pozostaje paczką, którą aplikacja dodaje do renderera. Sam zależy od `pathon-api` oraz od `pack-lib` `pathon-mesh` i `pathon-scene`. Nie rysuje sceny ani nie liczy oświetlenia: rysuje tylko głębię rzucających cień i oddaje mapę oraz uniformy. PCSS / VSM i cienie kontaktowe są odłożone z uzasadnieniem w ADR; światła obszarowe mają cień tylko z punktu centralnego i tylko na życzenie hosta ([niżej](#światła-punktowe-i-reflektory-k03)).

```rust,ignore
use pathon_shadow::{DirectionalShadowLight, ShadowPack, ShadowScene, ShadowSettings};

let mut renderer = Renderer::builder()
    .with_pack(ShadowPack::with_settings(ShadowSettings::DESKTOP))
    .with_pack(my_lighting_pack)   // czyta SHADOW_MAP i ShadowFrame
    .build()?;

// Co klatkę, przed render_frame:
let scene = renderer.shared_mut().get_mut::<ShadowScene>().expect("ShadowPack wstawia scenę");
scene.clear_casters();
scene.fill_from_world(&render_world, &geometry_pool);   // instancje z CASTS_SHADOWS
scene.light = Some(DirectionalShadowLight::new(sun_direction, sun_light_index));
```

## Wejście: `ShadowScene`

Typ w `Shared`, który host (albo paczka oświetlenia) wypełnia przed każdą klatką. Bez `light` nie ma cieni, slotu ani `ShadowFrame`.

| Element | Znaczenie |
| --- | --- |
| `DirectionalShadowLight { direction, light_index }` | kierunek, w którym światło świeci; `light_index` to numer, pod którym shader zna to światło (`DirectionalShadowLight::from_light(&Light, index)` czyta kierunek z `pathon-scene`) |
| `ShadowCaster` | jedno rysowanie: bufor pozycji (pierwszy strumień chunka, trzy `f32`), bufor indeksów `u32`, zakres indeksów, `base_vertex`, macierz modelu i pudełko świata (`Aabb::UNBOUNDED` = rysuj do każdej kaskady) |
| `add_mesh(pool, mesh, model)` | `ShadowCaster` z meshu `GeometryPool`; pudełko świata z granic meshu |
| `fill_from_world(world, pool)` | zastępuje rzucających widocznymi instancjami z flagą `CASTS_SHADOWS`; `MeshHandle` czytany jako `MeshId` puli (`to_id`) |
| `use_first_shadow_light(world)` | pierwsze światło kierunkowe z `casts_shadows`; indeks = pozycja w iteracji `RenderWorld::lights` |

## Ustawienia: `ShadowSettings` (w `Shared`, zmienne między klatkami)

| Pole | Domyślnie (`DESKTOP`) | Znaczenie |
| --- | --- | --- |
| `cascade_count` | 4 | 1…4 (i do `max_texture_array_layers`) |
| `resolution` | 2048 | bok mapy kaskady, 64…8192 (i do limitu tekstury) |
| `max_distance` | 150 m | za tym nie ma cieni; zanik na ostatnich `fade_fraction` (0,1) |
| `splits` | `Practical { lambda: 0.6 }` | `lambda` 0 = podział równomierny, 1 = logarytmiczny; `Fractions([a, b, c])` podaje ułamki wprost |
| `fit` | `Stable` | `Stable`: kula + przyciąganie do tekseli (bez pływania); `Focused`: ciasne pudełko (więcej rozdzielczości, krawędzie migoczą przy ruchu) |
| `filter` | `Pcf3` | `Hard` (1 tap), `Pcf3` (4 tapy, filtr pudełkowy 3 tekseli), `Pcf5` (16 próbek Poissona, 2,5 tekseli) |
| `rotate_filter` | tak | obrót dysku `Pcf5` szumem per piksel |
| `constant_bias` | 0,002 m | przesunięcie rzucających cień od światła (w macierzy passu głębi) |
| `normal_bias` | 1,0 tekseli | przesunięcie odbiorcy wzdłuż normalnej razy `sin(theta)` |
| `polygon_offset_constant`, `polygon_offset_slope` | 1,0 / 1,5 | sprzętowy `DepthBias` passu głębi (znak odwracany dla reverse-Z) |
| `blend_fraction` | 0,1 | mieszanie sąsiednich kaskad na końcu zakresu |
| `unbounded_caster_distance` | 200 m | jak daleko przed kaskadą może stać rzucający bez granic |
| `use_indirect_draws` | tak | `multi_draw_indexed_indirect` przy `INDIRECT_FIRST_INSTANCE`, inaczej pętla `draw_indexed` |

`ShadowSettings::COMPACT` (3 kaskady po 1024, 80 m) jest dla Web i mobile. Wartość poza zakresem jest naprawiana i zgłaszana raz jako [D0001](../diag/D0001.md).

## Podział i dopasowanie kaskad (`pathon_shadow::cascade`)

Czysty kod CPU, testowany bez GPU. `CameraFrustum::from_camera` odczytuje z macierzy projekcji kąty widzenia oraz near i far (perspektywa i ortho, reverse-Z; inaczej [D0003](../diag/D0003.md)). `split_distances` daje `n + 1` odległości widoku, `plan_cascades` macierze: `view_projection` (próbkowanie: x i y w −1…1, z = głębia reverse-Z, 1 przy świetle) i `caster_view_projection` (to samo z rzucającymi odsuniętymi o `constant_bias`). Przestrzeń światła ma osie (prawo, góra, przód) z przodem = kierunek światła; oś pomocnicza to Y, a przy świetle niemal pionowym X.

- **Brak dziur:** pierwsza kaskada zaczyna się w near kamery, każda następna tam, gdzie kończy się poprzednia, ostatnia w `max_distance`; test sprawdza, że każdy punkt plasterka rzutuje się do swojej mapy (x, y w 0…1, głębia w 0…1) dla `Stable` i `Focused` oraz dla kamery ortho.
- **Stabilizacja:** promień kuli zależy tylko od obiektywu i dwóch odległości; środek jest zaokrąglany do siatki tekseli światła, więc ruch i obrót kamery przesuwają mapę o całe teksele (test: przesunięcie punktu w tekselach jest całkowite z dokładnością 0,02, a rozmiar teksela się nie zmienia).
- **Zakres głębi:** do najbliższego rzucającego cień, który zachodzi na prostokąt; dalsi rzucający nie rozszerzają go.

## Wyjście: slot, `ShadowFrame`, WGSL

- Slot `SHADOW_MAP` (`shadow.map`, `Depth32Float`, jedna warstwa na kaskadę). Czytaj go przez `pass.read(map, SHADOW_MAP_ACCESS)` (`SAMPLED | DEPTH_READ`) i widok `pass.view_with(map, &ShadowBindings::map_view_desc())` (`D2Array`).
- `ShadowFrame` w `Shared` (nowy co klatkę, gdy jest światło): `uniforms` (`ShadowUniforms`, 368 B, układ WGSL `ShadowCascades`), `uniform_buffer` (`UniformSlice` w pierścieniu tej klatki), `sampler` porównawczy, `cascade_count`, `resolution`.
- `ShadowBindings::starting_at(n)`: `wgsl_declarations(group)` (`shadow_cascades`, `shadow_map`, `shadow_sampler` na bindingach `n`…`n+2`), `layout_entries(visibility)` i `bind_group_entries(&frame, map_view)`.
- Moduł `package::shadow::sample` (`pathon_shadow::modules()` do `ShaderLibrary::add_module`):

| Element | Znaczenie |
| --- | --- |
| `struct ShadowCascades` | uniform kaskad: macierze, rozmiar teksela, bias normalny, `splits` (koniec każdej kaskady jako odległość widoku), liczba kaskad, indeks światła, filtr, parametry mieszania i zaniku |
| `shadow_factor_directional(shadows, map, sampler, light_index, world_position, normal, view_depth, pixel) -> f32` | 0 w cieniu, 1 oświetlone; `normal` to normalna geometryczna (jednostkowa), `view_depth` odległość wzdłuż osi kamery w metrach, `pixel` to `@builtin(position).xy` (tylko do szumu filtra) |
| `shadow_cascade_index(shadows, view_depth)` | numer kaskady dla odległości |
| `shadow_light_space`, `shadow_pcf3`, `shadow_pcf5`, `shadow_sample_cascade` | elementy składowe |

`pathon-pbr` wywołuje `shadow_factor_directional` w pętli świateł (permutacja `shadows`, [pathon-pbr](pathon-pbr.md#cienie)) z indeksem światła w swojej tablicy (światła kierunkowe pierwsze) jako `light_index`; normalną, głębię widoku i `pixel` bierze z kontekstu fragmentu.

```wgsl
import package::shadow::sample::{ShadowCascades, shadow_factor_directional};
SHADOW_DECLARATIONS   // ShadowBindings::starting_at(3).wgsl_declarations(0)

let visibility = shadow_factor_directional(shadow_cascades, shadow_map, shadow_sampler,
    light_index, in.world_position, normal, view_depth, in.clip.xy);
let radiance = n_dot_l * light_color * visibility;
```

## Światła punktowe i reflektory (K03)

Powód: słońce było jedynym światłem z cieniem, a w pomiarze K01 w zacienionej połowie pikseli classroom pathon dawał 1,8 do 2,6 razy więcej światła bezpośredniego niż Cycles. Decyzje i alternatywy (atlas zamiast tablicy warstw i `CubeArray`, sześć ścian z pasem ochronnym zamiast podwójnego paraboloidu, pamięć podręczna po sygnaturze, światła obszarowe): [ADR-0050](../adr/0050-cienie-swiatel-punktowych-i-reflektorow.md).

```rust,ignore
let scene = renderer.shared_mut().get_mut::<ShadowScene>().expect("ShadowPack wstawia scenę");
scene.use_local_lights(&render_world, LocalLightFilter::default());   // światła z `casts_shadows`
// albo ręcznie: scene.add_local_light(LocalShadowLight { key, light_index, shape, position, direction, range, intensity });
```

**Wejście.** `ShadowScene::local_lights` to *kandydaci*: `ShadowSettings::max_local_lights` najważniejszych dla widoku dostaje mapy, reszta świeci bez cienia. `LocalShadowLight` ma `key` (tożsamość między klatkami; mapa żyje, dopóki klucz jest oferowany), `light_index` (indeks w tablicy świateł `pathon-pbr`: kierunkowe pierwsze, potem pozostałe w kolejności `RenderWorld::lights`; `use_local_lights` liczy go tak jak `PackedLights`), kształt (`Point` albo `Spot { outer_angle }`, połowa kąta do 80°, szerszy stożek dostaje mapę 160° i poza nią jest oświetlony), pozycję, kierunek, zasięg (mapa sięga tak daleko) i `intensity` (waga rankingu w dowolnej wspólnej jednostce, naturalnie kandele razy maksimum koloru). `LocalLightFilter { only_casting, area_as_point }` mówi, które światła sceny wchodzą; `invalidate_local_maps()` wymusza narysowanie wszystkiego od nowa, gdy treść siatki zmieniła się w miejscu.

| Ustawienie | Domyślnie (`DESKTOP` / `COMPACT`) | Znaczenie |
| --- | --- | --- |
| `max_local_lights` | 8 / 4 | ile świateł ma mapy naraz (0 wyłącza, do 16) |
| `local_atlas_size` | 4096 / 2048 | bok atlasu (potęga dwójki, 256 do 8192; 4096 to 64 MiB) |
| `local_spot_resolution`, `local_point_resolution` | 1024, 512 / 512, 256 | bok mapy reflektora i jednej ściany światła punktowego w odległości do `local_reference_distance` |
| `local_min_resolution` | 128 | najmniejszy bok |
| `local_reference_distance` | 6 m | do tej odległości od kamery pełna rozdzielczość, dalej połowa przy każdym podwojeniu |
| `local_constant_bias`, `local_relative_bias` | 0,01 m, 0,002 | przesunięcie odbiorcy ku światłu (stałe i jako część odległości) |
| `local_normal_bias` | 1,5 tekseli | normal offset odbiorcy |
| `local_stickiness` | 0,25 | o tyle mocniej liczy się światło, które już ma mapę |

Filtr (`filter`, `rotate_filter`) i offset wielokąta (`polygon_offset_*`) są wspólne z kaskadami.

**Mapy.** Jedna tekstura `Depth32Float`, kafle kwadratowe o boku będącym potęgą dwójki przydzielane przez alokator buddy. Reflektor ma jeden kafel (perspektywa nad stożkiem), światło punktowe sześć (ściany +X, −X, +Y, −Y, +Z, −Z). Każdy kafel jest renderowany z polem widzenia szerszym o pas 4 tekseli, więc filtr nigdy nie czyta sąsiedniego kafla, a ściana jest wybierana w shaderze po największej składowej wektora od światła: żadnego próbkowania sześcianu sprzętowo i żadnego szwu. Głębia to `near / d` (d wzdłuż osi kafla, nieskończona płaszczyzna far, reverse-Z); bliska płaszczyzna to `0,002 · zasięg` w granicach 2 do 20 cm.

**Wybór świateł.** Światło poza frustum kamery (kula zasięgu, dla reflektora kula opisana na stożku) nie dostaje mapy i nie jest rysowane. Pozostałe są szeregowane wagą `intensity · min(1, (zasięg / odległość)²)`; światło, które ma już mapę, liczy się `local_stickiness` mocniej, więc dwa podobne światła nie zamieniają się mapami przy każdym ruchu kamery. Rozdzielczość maleje z odległością jak w tabeli; zmiana stopnia ma histerezę (trzy czwarte stopnia), bo każda zmiana rozmiaru to nowa mapa. Gdy brakuje miejsca w atlasie, najpierw znikają mapy, o które nikt nie prosi (mapa światła, które odeszło z widoku, zostaje w atlasie do 240 klatek i wraca bez rysowania), potem kafle kolejnych świateł maleją, w końcu światło traci mapę ([D0101](../diag/D0101.md)).

**Pamięć podręczna.** Każdy kafel ma sygnaturę: miejsce w atlasie, macierz, a dla każdego rysowanego rzucającego bufory, zakres indeksów i macierz modelu. Gdy sygnatura zgadza się z zeszłą klatką, kafel nie jest rysowany (atlas jest trwały: paczka trzyma teksturę i importuje ją do grafu co klatkę). Przesunięcie światła albo rzucającego w zasięgu odświeża tylko jego kafle, rzucający poza zasięgiem nic nie kosztuje. Sygnatura zastępuje oznaczanie instancji jako statycznych i dynamicznych: statyczny rzucający nie zmienia macierzy, więc nie zmienia sygnatury, a ruchomy zmienia ją sam. Kafel do narysowania ustawia viewport i scissor, czyści swój prostokąt trójkątem na płaszczyźnie far i rysuje rzucających; wszystkie kafle klatki są w jednym passie `shadow local`. Paczka nie dowiaduje się o porzuceniu klatki (błąd z `render_frame` po jej nagraniu), więc host po takim błędzie woła `ShadowScene::invalidate_local_maps()`.

**Bias** (cztery warstwy): offset wielokąta w passie (wspólny z kaskadami), normal offset odbiorcy o `sin(theta) · local_normal_bias · teksel(odległość)` (teksel rośnie z odległością od światła, więc offset też), stała `local_constant_bias` i część odległości `local_relative_bias` przesuwające odbiorcę ku światłu (precyzja głębi jest względna).

**WGSL** (`package::shadow::sample`): struktury `LocalShadows` (uniform, 5680 B), `shadow_local_tile_index`, `shadow_local_visibility` (filtry `Hard`, `Pcf3`, `Pcf5` nad atlasem, jak w kaskadach) i `shadow_local_face`. Moduł nie ma bindingów, a duży uniform nie przechodzi do funkcji przez wartość: shader deklaruje bindingi razem z funkcją pomocniczą `local_shadow_factor(light_index, world_position, normal, pixel) -> f32` (0 w cieniu, 1 oświetlone i dla światła bez mapy), którą dają `LocalShadowBindings::wgsl_bindings(group)` i `LocalShadowBindings::wgsl_helper()` (razem `wgsl_declarations`) i która woła funkcje modułu na zmiennych globalnych. Trzy kolejne bindingi: uniform, atlas `texture_depth_2d`, sampler porównawczy.

```wgsl
import package::shadow::sample::{LocalShadows, shadow_local_tile_index, shadow_local_visibility};
LOCAL_SHADOW_DECLARATIONS   // LocalShadowBindings::starting_at(n).wgsl_declarations(0)

let visibility = local_shadow_factor(light_index, in.world_position, normal, in.clip.xy);
```

Wyjście do `Shared`: `LocalShadowFrame` (miejsce uniformów w pierścieniu, sampler, liczba świateł; tylko gdy jakieś światło ma mapę), slot `SHADOW_LOCAL_MAP` (`shadow.local`: czytaj `SHADOW_MAP_ACCESS` i widokiem `LocalShadowBindings::atlas_view_desc()`) oraz `LocalShadowReport` (decyzje klatki: światła z mapami i rozmiarami, światła poza widokiem, bez miejsca, narysowane i zachowane kafle, liczba narysowanych rzucających).

**Integracja z `pathon-pbr`.** Permutacja z flagą WESL `local_shadows` (`ForwardKey::local_shadows`) dodaje trzy bindingi od `LOCAL_SHADOW_FIRST_BINDING` (za bindingami wszystkich pozostałych flag, więc inne permutacje mają układ jak dawniej); `add_light` mnoży promieniowanie punktowego i reflektora przez `local_shadow_factor(index, …)`, gdzie `index` to indeks z tablicy świateł. Bez `LocalShadowFrame` albo bez slotu działa permutacja bez cieni lokalnych.

**Światła obszarowe.** `LocalLightFilter::area_as_point` oferuje je jako światła punktowe w środku prostokąta, a `add_area_light` w pbr mnoży wynik LTC przez ten sam współczynnik. To przybliżenie (bez penumbry, błędne przy końcach długiego źródła), dlatego domyślnie wyłączone; pomiar na classroom: [porównanie obrazu](../porownanie-obrazu.md#k03-cienie-świateł-punktowych-i-reflektorów).

**Ograniczenia.** Wybór świateł zależy od kamery pierwszego widoku klatki; kolejne widoki czytają te same mapy. Rzucające z maską alfa nie mają jeszcze wariantu (jak w kaskadach). Mapy nie są dzielone na warstwę statyczną i dynamiczną: ruchomy rzucający w zasięgu światła odświeża całe jego kafle.

## Diagnostyki

| Kod | Sytuacja |
| --- | --- |
| [D0001](../diag/D0001.md) | ustawienie poza zakresem naprawione |
| [D0002](../diag/D0002.md) | kierunek światła zerowy albo nieskończony; brak cieni w tej klatce |
| [D0003](../diag/D0003.md) | projekcja kamery nie jest perspektywą ani ortho z reverse-Z; brak cieni w tej klatce |
| [D0101](../diag/D0101.md) | światło punktowe lub reflektor nie zmieściło się w atlasie map |
| [D0102](../diag/D0102.md) | światło punktowe lub reflektor niepoprawne (niefinitne albo niedodatnie wartości), pominięte |

## Wiele widoków i utrata urządzenia

Bufory paczki (macierze modeli, argumenty indirect, uniformy) są zapisywane przy nagrywaniu: każda próba nagrania zaczyna od początku, a kolejne widoki tej samej klatki dopisują za poprzednimi (test z dwoma widokami). `on_device_reset` tworzy wszystko od nowa i usuwa `ShadowFrame`.

## Testy i przykład

- Jednostkowe i `tests/shaders.rs` (bez GPU): podział kaskad, macierze i pokrycie, stabilizacja do tekseli, ustawienia, plan rysowania i culling, moduł WESL składa się z shaderem oświetlenia i przechodzi walidację, bindingi zgadzają się z layoutem.
- `tests/pack.rs` (`rhi-null`): pełny renderer z walidacją nagranych komend w obu trybach rysowania, `D0001`…`D0003`, dwa widoki w klatce, brak światła.
- `tests/golden_shadows.rs` (`--features gpu-tests`, `PATHON_TEST_BACKEND=vulkan|dx12|metal`): scena z podłogą, stosem sześcianów, kulą, płaszczyzną skierowaną do światła i odległym sześcianem; shading `N·L × shadow_factor` napisany w teście. Asercje: piksel w cieniu ≪ w świetle (pięć rzucających, w tym kaskada 3), brak acne na płaszczyźnie prostopadłej do światła (najciemniejszy z 81 próbek ≥ 97% wartości) i na podłodze pod kątem przez wszystkie kaskady (68 próbek, równe co do bitu), brak peter-panningu (cień sięga 3 cm od ściany sześcianu), mapa przesuwa się o całe teksele przy ruchu i obrocie kamery (`Stable`; `Focused` migocze), miększe filtry poszerzają krawędź, rysowanie indirect i pojedyncze dają ten sam obraz. Golden image `shadows_csm` w `tests/golden/`.
- Światła punktowe i reflektory: jednostkowe w `src` (alokator atlasu, culling i wybór z histerezą, rozdzielczość od odległości, pamięć podręczna po sygnaturze, kolejność ścian, bryła stożka, indeksy jak w pbr, ustawienia); `tests/pack_local.rs` (`rhi-null`: pełny renderer z `LocalShading` w teście, walidacja barier, bind groupów i argumentów indirect dla trwałego atlasu, tylko zmienione kafle rysowane ponownie, `invalidate_local_maps`, światło poza widokiem bez mapy, limit `max_local_lights`, `D0101` i `D0102` raz, dwa widoki); `tests/golden_local.rs` (`--features gpu-tests`): reflektor za sześcianem (piksel w cieniu 0, w świetle 1), brak acne na podłodze pod kątem 14° (675 punktów), cień sięga 1 cm od ściany sześcianu, światło punktowe w pokoju z sześciu stron (8961 odbiorników mierzonych sześcioma kamerami względem promienia: 0 przecieków i 0 acne), cień sześcianu stojącego na szwie ścian (80 odbiorników przy szwie, wszystkie w cieniu), niezmieniona klatka daje ten sam obraz bez rysowania, przesunięte światło daje obraz równy świeżemu rendererowi, rysowanie indirect i pojedyncze są równe, filtry poszerzają krawędź, golden `shadows_local` (reflektor i dwa światła punktowe).
- Przykład: `cargo run -p pathon-examples --bin shadow_scene -- wynik.png [hard|pcf3|pcf5] [stable|focused] [kaskady]` ([przykłady](../examples.md)).

**Nie potwierdzone / otwarte:** referencja na lavapipe, WARP i Metalu dopiero w CI; ścieżka bez `INDIRECT_FIRST_INSTANCE` była sprawdzona tylko na RX 9070 XT (wymuszona `use_indirect_draws = false`) i na `rhi-null`, nie w przeglądarce; od przepięcia viewera sceny testowej (docs/scenes.md) cienie przechodzą też przez `pathon-pbr` (testy w `pathon-pbr/tests/golden_scene.rs`).
