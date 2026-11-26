# pathon-gi

Globalne oświetlenie z sond irradiancji (K12, [ADR-0049](../adr/0049-gi-sondy-irradiancji.md)): regularna siatka sond w bryle sceny, każda widzi scenę przez śledzenie promieni w siatce wokseli i zapisuje irradiancję i rozkład odległości do najbliższych powierzchni (wzorzec DDGI: Majercik i in. 2019, 2021, ale bez sprzętowego śledzenia promieni, tylko compute). Shader oświetlenia czyta wynik funkcją WGSL `gi_sample` / `gi_irradiance`: diffuse z odbić światła wewnątrz sceny i nieba widzianego przez otwory zamiast płaskiego ambientu IBL, który nie wie o ścianach.

To `pack-lib` (`[package.metadata.pathon] kind = "pack-lib"`, [ADR-0041](../adr/0041-pack-lib.md), jak `pathon-shadow` i `pathon-sky`): `PbrPack` zależy od niego, żeby czytać `GiFrame` i moduł WGSL, a `GiPack` pozostaje paczką, którą aplikacja dodaje do renderera. Sam zależy od `pathon-api`, `pathon-shader` i `pack-lib` `pathon-mesh`, `pathon-scene`, `pathon-material`, `pathon-bindless`, `pathon-ibl`. Nie rysuje obrazu i nie zna `pathon-pbr`.

```rust,ignore
use pathon_gi::{GiPack, GiSettings};

let mut renderer = Renderer::builder()
    .with_pack(IblPack::new())
    .with_pack(GiPack::with_settings(GiSettings::default()))   // przed PbrPack nie trzeba: cecha ma Stage::Setup
    .with_pack(PbrPack::new())
    .build()?;
// Wejścia te same, które czyta PbrPack: Snapshot (co klatkę), GeometryPool, MaterialTable, opcjonalnie TextureBinder.
// Skala nieba widzianego przez sondy ma być taka jak PbrSettings::environment_intensity.
renderer.shared_mut().get_mut::<GiSettings>().unwrap().environment_intensity = 300.0;
```

Bez `GiPack` (albo z `GiSettings::enabled = false`) `PbrPack` działa dokładnie jak dotąd: permutacja shadera bez flagi `gi` ma ten sam układ bindingów co przed K12.

## Jak to działa

Każda klatka (cecha `gi probes`, `Stage::Setup`, pierwszy widok klatki; kolejne widoki tylko importują atlasy):

| Pass | Co robi | Koszt (classroom 1080p, RX 9070 XT, domyślnie) |
| --- | --- | --- |
| `gi voxelize` | tylko gdy scena się zmieniła: czyści siatkę, woksalizuje każdą instancję (jeden wątek na trójkąt, pełny test trójkąt-sześcian, 13 osi), z albedo = czynnik materiału razy tekstura bazowa próbkowana mipem pokrywającym komórkę, emisja, potem lista komórek stałych | jednorazowo (osobno niemierzone; classroom: 606 tys. trójkątów, 846 rysowań) |
| `gi light voxels` | oświetla ściany komórek stałych: światło bezpośrednie wszystkich świateł (cień z samych wokseli), emisja i światło sond sprzed klatki jako drugie i kolejne odbicia; jedna `period`-ta część komórek na klatkę | 1,0 ms |
| `gi trace probes` | jeden wątek na promień sond tej klatki: DDA (Amanatides i Woo) po wokselach, trafienie daje radiancję ściany, ucieczka z siatki daje środowisko IBL | 0,22 ms |
| `gi update probes` | składa promienie w irradiancję (oktaedr 8×8, kosinusowo ważona średnia, w alfie widoczność nieba) i w momenty odległości (oktaedr 16×16, średnia i średnia kwadratów), z histerezą czasową; atlasy są dwie (czytana i pisana), w każdej tekstura `Rgba16Float` z ramką jednego teksela | 0,48 ms |

Wynik czyta `pbr forward` (dodatkowe ok. 0,5 ms w tej scenie): `gi_sample` miesza osiem sond wokół punktu trójliniowo, z wagą wygładzoną od tyłu sondy i testem Czebyszewa na momentach odległości (sonda za ścianą ma średnią odległość krótszą niż odległość do punktu, więc jej waga idzie do zera: stąd brak wycieku światła). Poza objętością i tam, gdzie żadna sonda nie widzi punktu, shader zostaje przy IBL (`confidence` = 0).

Konwencja jednostek: irradiancja to **kosinusowo ważona średnia radiancja** (E/π), jak `ibl_diffuse`; diffuse = albedo × irradiancja. W jednostkach sceny (lux, kandele, emisja ×683) tak samo jak światła `PbrPack`.

## Wejście i wyjście

| Typ | Rola |
| --- | --- |
| `GiSettings` (`Shared`) | patrz niżej; zmiana pól objętości buduje nową objętość i zaczyna sondy od początku |
| `GiFrame` (`Shared`, co klatkę, gdy jest objętość) | uniform objętości (`GiVolumeUniforms`, 80 B), sampler, uchwyty grafu obu atlasów; nie trzymać między klatkami |
| `GiStatus` (`Shared`) | liczby ostatniej klatki: sondy, promienie, woksele, pamięć (benchmark viewera wypisuje je pod tabelą) |
| `GiBindings::starting_at(n)` | cztery kolejne bindingi (uniform, dwa atlasy, sampler): `wgsl_declarations`, `layout_entries`, `bind_group_entries` |
| moduły WGSL `gi::common`, `gi::sample` (`modules()`) | `gi_sample(volume, irradiance, visibility, sampler, position, normal, view, specular_direction) -> GiSample { irradiance, confidence, sky_visibility }` i skrót `gi_irradiance(...) -> vec3` |

W `PbrPack` (`pathon-pbr`): flaga shadera `gi` (`ForwardKey::gi`), bindingi od `GI_FIRST_BINDING` (po tablicy LTC). `diffuse_środowiska = mix(IBL, irradiancja_sond × albedo_dyfuzyjne × (1 − odbicie_spekularne), confidence)`, a specular IBL jest mnożone przez `mix(1, widoczność_nieba, confidence × specular_occlusion)`: środowisko dochodzi tylko tam, dokąd dochodzi niebo.

## Ustawienia (`GiSettings`)

| Pole | Domyślnie | Znaczenie |
| --- | --- | --- |
| `enabled` | tak | wyłączone: żadnych passów, nic nie jest publikowane |
| `bounds` | z geometrii | pudełko objęte sondami i wokselami; `None` bierze pudełko woksalizowanych instancji |
| `probe_spacing`, `max_probes` | 0,5 m, 4096 | odstęp sond; rośnie, gdy sond byłoby więcej niż `max_probes` |
| `voxel_size`, `max_voxels` | 0,1 m, 3 mln | krawędź wokselu: najmniejsza szczelina, przez którą promień coś widzi; rośnie, gdy brakuje budżetu (max 4 mln) |
| `rays_per_probe`, `probes_per_frame` | 128, wszystkie | promienie na sondę (1…256); sondy w paczkach po `probes_per_frame`, okrężnie |
| `hysteresis` | 0,95 | waga poprzedniej wartości sondy; pierwsze 8 klatek po zbudowaniu objętości rośnie od 0, więc objętość nie startuje ciemna |
| `depth_sharpness` | 12 | wykładnik wagi kosinusowej momentów odległości |
| `max_ray_distance` | 40 m | dalej jest środowisko; momenty saturują |
| `normal_bias`, `view_bias` | 0,1 m, 0,05 m | przesunięcie punktu zapytania przeciw „wciąganiu” światła ze ściany, za którą stoi sonda |
| `voxel_relight_period` | 4 | co ile klatek oświetlana jest każda komórka (przez 8 pierwszych klatek: co klatkę) |
| `bounce_intensity` | 1 | skala światła sond wracającego do wokselów (drugie i kolejne odbicia; 0 = jedno odbicie) |
| `indirect_intensity` | 1 | skala światła pośredniego, które dostają powierzchnie |
| `environment_intensity` | 1 | skala środowiska widzianego przez promienie: ustaw jak `PbrSettings::environment_intensity` |
| `specular_occlusion` | 1 | ile widoczności nieba z sond zaciemnia specular IBL |

`GiSettings::COMPACT` to mała objętość na Web i słabe GPU (1 m, 64 promienie, woksele 20 cm). Wartość spoza zakresu zastępuje całe ustawienie domyślnym i daje `J0001`.

## Co jest woksalizowane, a co nie

Woksalizowane są widoczne instancje z materiałem nieprzezroczystym (maska alfa liczy się jako pełne) o transmisji do 0,5; **szkło i materiały z blendem nie zatrzymują promieni**. Geometria jest brana w pozie bazowej (skórka i morfy: bez deformacji), a **zmiana dowolnej instancji, materiału lub transformacji buduje siatkę od nowa** (skrót po instancjach, materiałach i transformacjach; cena jak pierwsze zbudowanie). Światła obszarowe (K05) są próbkowane siatką emiterów kosinusowych (co 0,7 m, najwyżej 12×12), bo promienie cienia z wokselów nie całkują po kształcie; światła kierunkowe, punktowe i reflektory mają te same wzory co `pbr::lights`. Inne rodzaje światła (nieznane tej wersji) są pomijane.

## Znane ograniczenia

- Cienkie elementy mniejsze od wokselu (żaluzje, szprosy) stają się pełnymi ścianami o grubości do wokselu: mniej światła pośredniego wpada przez okna classroom (mierzone w [porownanie-obrazu.md](../porownanie-obrazu.md#k12-sondy-irradiancji)); woksel 5 cm poprawia to kosztem pamięci (112 MiB) i oświetlenia komórek (2 ms przy okresie 8).
- Ściany pełne woksalizują się o do jednego wokselu grubiej do wnętrza: pudełko Cornella wychodzi o ok. 8% jaśniejsze niż w teorii (test z Monte Carlo: bias +8,3%, najgorszy kanał 12,6%).
- Objętość nie podąża za kamerą: jedna regularna siatka na całą scenę (budżet sond i wokselów rośnie z jej rozmiarem).
- Obiekty ruchome: pełna ponowna woksalizacja zamiast aktualizacji przyrostowej.
- Koszt CPU: ok. 1,1 ms na klatkę przy 862 instancjach (zbieranie sceny, sygnatura, światła), do wyniesienia za licznik epoki sceny.

## Kody diagnostyk

`J0001` ustawienia, `J0002` brak wejścia w `Shared`, `J0003` zły układ puli geometrii, `J0004` nic do woksalizacji, `J0005` urządzenie bez storage `Rgba16Float` lub grup po 64, `J0006` błąd shadera ([docs/diag](../diag/J0001.md)).

## Testy

`crates/pathon-gi/tests/probes.rs` (null device zawsze, GPU z `--features gpu-tests`): biały piec (zamknięty pokój o emitujących ścianach: średnia 1,975 przy 2,0, najgorsza sonda 1,5%), brak wycieku przez ścianę (ciemny pokój 3·10⁻⁴ przy jasnym 1,96), pudełko Cornella względem Monte Carlo CPU (`reference::Room`), brak NaN, stabilność czasowa (zmiana średniej irradiancji po 30 klatkach 0,6%). Golden `tests/golden/gi_cornell.png` i test efektu w `examples/tests/gi_cornell.rs`, permutacja `gi` w `pathon-pbr/tests/shaders.rs`.

Przykład: `cargo run --release -p pathon-examples --bin gi_cornell` (dwa PNG: z samym środowiskiem i z GI); `gltf_viewer --gi` (opcje: `--gi-spacing`, `--gi-voxel`, `--gi-rays`, `--gi-hysteresis`, `--gi-period`, `--gi-warmup`, [examples.md](../examples.md)).
