# pathon-shadow

Cienie światła kierunkowego z map kaskadowych (M4-07, [ADR-0044](../adr/0044-cienie-kaskadowe-csm.md)): od 1 do 4 kaskad podzielonych wzdłuż głębi kamery, każda w warstwie jednej tekstury `Depth32Float` (reverse-Z), filtr PCF, stabilizacja do tekseli i trzy warstwy biasu. Pass cieni jest węzłem grafu w `Stage::Shadows` i trzyma rolę `Role::SHADOWS`; shader oświetlenia czyta wynik funkcją WGSL `shadow_factor_directional`.

To `pack-lib` (`[package.metadata.pathon] kind = "pack-lib"`, [ADR-0041](../adr/0041-pack-lib.md)): `PbrPack` zależy od niego, żeby czytać `ShadowFrame`, slot i moduł WGSL, a `ShadowPack` pozostaje paczką, którą aplikacja dodaje do renderera. Sam zależy od `pathon-api` oraz od `pack-lib` `pathon-mesh` i `pathon-scene`. Nie rysuje sceny ani nie liczy oświetlenia: rysuje tylko głębię rzucających cień i oddaje mapę oraz uniformy. Spoty i punktowe (kostka), PCSS / VSM i cienie kontaktowe są odłożone z uzasadnieniem w ADR.

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

## Diagnostyki

| Kod | Sytuacja |
| --- | --- |
| [D0001](../diag/D0001.md) | ustawienie poza zakresem naprawione |
| [D0002](../diag/D0002.md) | kierunek światła zerowy albo nieskończony; brak cieni w tej klatce |
| [D0003](../diag/D0003.md) | projekcja kamery nie jest perspektywą ani ortho z reverse-Z; brak cieni w tej klatce |

## Wiele widoków i utrata urządzenia

Bufory paczki (macierze modeli, argumenty indirect, uniformy) są zapisywane przy nagrywaniu: każda próba nagrania zaczyna od początku, a kolejne widoki tej samej klatki dopisują za poprzednimi (test z dwoma widokami). `on_device_reset` tworzy wszystko od nowa i usuwa `ShadowFrame`.

## Testy i przykład

- Jednostkowe i `tests/shaders.rs` (bez GPU): podział kaskad, macierze i pokrycie, stabilizacja do tekseli, ustawienia, plan rysowania i culling, moduł WESL składa się z shaderem oświetlenia i przechodzi walidację, bindingi zgadzają się z layoutem.
- `tests/pack.rs` (`rhi-null`): pełny renderer z walidacją nagranych komend w obu trybach rysowania, `D0001`…`D0003`, dwa widoki w klatce, brak światła.
- `tests/golden_shadows.rs` (`--features gpu-tests`, `PATHON_TEST_BACKEND=vulkan|dx12|metal`): scena z podłogą, stosem sześcianów, kulą, płaszczyzną skierowaną do światła i odległym sześcianem; shading `N·L × shadow_factor` napisany w teście. Asercje: piksel w cieniu ≪ w świetle (pięć rzucających, w tym kaskada 3), brak acne na płaszczyźnie prostopadłej do światła (najciemniejszy z 81 próbek ≥ 97% wartości) i na podłodze pod kątem przez wszystkie kaskady (68 próbek, równe co do bitu), brak peter-panningu (cień sięga 3 cm od ściany sześcianu), mapa przesuwa się o całe teksele przy ruchu i obrocie kamery (`Stable`; `Focused` migocze), miększe filtry poszerzają krawędź, rysowanie indirect i pojedyncze dają ten sam obraz. Golden image `shadows_csm` w `tests/golden/`.
- Przykład: `cargo run -p pathon-examples --bin shadow_scene -- wynik.png [hard|pcf3|pcf5] [stable|focused] [kaskady]` ([przykłady](../examples.md)).

**Nie potwierdzone / otwarte:** referencja na lavapipe, WARP i Metalu dopiero w CI; ścieżka bez `INDIRECT_FIRST_INSTANCE` była sprawdzona tylko na RX 9070 XT (wymuszona `use_indirect_draws = false`) i na `rhi-null`, nie w przeglądarce; od przepięcia viewera sceny testowej (docs/scenes.md) cienie przechodzą też przez `pathon-pbr` (testy w `pathon-pbr/tests/golden_scene.rs`).
