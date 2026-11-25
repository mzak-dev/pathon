# pathon-sky

Niebo (M4-08, [ADR-0045](../adr/0045-niebo-atmosfera-hillaire.md)): fizyczna atmosfera Hillaire 2020 rysowana do slotu `hdr` tam, gdzie nie ma geometrii, słońce publikowane jako `SunLight` w `Shared` i to samo niebo jako źródło środowiska dla [`pathon-ibl`](pathon-ibl.md). Jeden model daje tło, ambient i światło słońca, więc pora dnia zmienia barwę wszystkich trzech naraz.

Crate ma `kind = "pack-lib"` ([ADR-0041](../adr/0041-pack-lib.md)): zależy od `pathon-api` i `pathon-ibl`, a `pathon-pbr` i `pathon-shadow` mogą czytać `SunLight` po typie. Nie rysuje geometrii, nie liczy cieni ani chmur (jest tylko proste zachmurzenie), nie czyta plików.

```rust,ignore
let renderer = Renderer::builder()
    .with_pack(IblPack::new())            // zbuduje środowisko z nieba
    .with_pack(SkyPack::new().with_settings(SkySettings::default().with_sun(35.0, 120.0)))
    .with_pack(PostPack::new())           // ekspozycja i tonemapping
    .build()?;

// Co klatkę (ustawienia czytane z Shared, zmiana działa od razu):
renderer.shared_mut().insert(SkySettings { cloud_coverage: 0.4, ..settings });
// Paczki oświetlenia czytają:
let sun = renderer.shared().get::<SunLight>();          // kierunek i lux per kanał
let environment = renderer.shared().get::<IblEnvironment>();
```

```rust
# use pathon_sky::{SkySettings, reference::{Planet, SkyConditions}, Atmosphere};
// Słońce 3° nad horyzontem: jego światło traci niebieski i zielony po drodze przez atmosferę.
let settings = SkySettings::default().with_sun(3.0, 0.0);
let planet = Planet::new(&Atmosphere::EARTH);
let sun = settings.sun_direction;
let conditions = SkyConditions {
    camera_radius_km: f64::from(settings.camera_radius_km(1.7)),
    sun_direction: [f64::from(sun.x), f64::from(sun.y), f64::from(sun.z)],
    sun_illuminance: [120_000.0; 3],
    cloud_coverage: 0.0,
};
let at_camera = pathon_sky::reference::sun_illuminance_at_camera(&planet, &conditions);
assert!(at_camera[0] > 3.0 * at_camera[2]);
```

## Co powstaje

Cztery przebiegi w `Stage::Sky`, co klatkę, wszystko w WGSL bez pamięci współdzielonej i atomików (Web działa tak samo); tablice to `Rgba16Float` zapisywane z compute i filtrowane przy próbkowaniu.

| Przebieg | Rozmiar (`DESKTOP`) | Zawartość |
| --- | --- | --- |
| `sky transmittance` | 256×64, 40 kroków | transmitancja do kosmosu w parametryzacji `(μ, r)` Bruneton-Neyret; slot `sky.transmittance` |
| `sky multiscatter` | 32×32, 64 kierunki × 20 kroków | `Ψ_ms`: rozpraszanie drugiego rzędu (faza izotropowa, odbicie od ziemi) z szeregiem wyższych rzędów; slot `sky.multiscatter` |
| `sky view` | 192×108, 32 kroki | luminancja nieba na jednostkę oświetlenia słońca z miejsca kamery: horyzont w połowie wysokości, gęstość przy azymucie słońca; slot `sky.sky_view` |
| `sky render` | rozmiar renderu | pełnoekranowy trójkąt: czyta sky-view i głębię, pisze `hdr` |

`SkyQuality::COMPACT` (Web, mobile) to czwarta część tekseli i około połowy kroków. Rozmiary są stałe po zbudowaniu rendera: `SkyPack::with_quality`.

Jednostki: długości w kilometrach, współczynniki na kilometr, oświetlenie w luksach, radiancja w nitach; `hdr` jest liniowe i bez ekspozycji ([docs/slots.md](../slots.md)). Zenit czystego nieba przy słońcu na 55° ma około 2000 nitów w zielonym kanale (`pathon-post` sprowadza to do wyświetlacza). Atmosfera to `Atmosphere::EARTH`; wszystkie współczynniki (Rayleigh, Mie, ozon, skale wysokości, anizotropia Mie, promienie) są w `SkySettings::atmosphere`.

## Wyjścia

**1. Tło w `hdr`.** Przebieg ładuje `hdr` i pisze tam, gdzie `Slot::DEPTH` jest zerem (reverse-Z); bez slotu głębi rysuje wszędzie, bez slotu `hdr` tworzy go i czyści. Poniżej horyzontu (gdy nic tam nie narysowano) widać ziemię: Lambert z `ground_albedo`, oświetlony słońcem przez atmosferę. Dysk słońca (`sun_angular_radius`, miękka krawędź) ma radiancję ograniczoną do `sun_disc_max_radiance` (50 000 nitów), bo prawdziwa (≈ 10⁹) nie mieści się w binary16 i zniszczyłaby bloom; `draw_sun_disc = false` wyłącza go.

**2. Źródło IBL.** `SkyEnvironmentFeature` (`Stage::Setup`, przed `ibl`) liczy niebo na CPU tym samym modelem co shadery (`pathon_sky::reference`), robi z niego `EquirectImage` (domyślnie 128×64, bez dysku słońca) i wkłada do `Shared` jako `IblSource`; `IblPack` buduje z niego prefiltr i irradiancję jak z każdego obrazu. `IblPack` trzeba dodać do rendera, `SkyPack` go nie rejestruje (może służyć też do pliku `.hdr`). Powody CPU (obraz w `IblSource` jest w pamięci, odczyt GPU nie jest przenośny) i alternatywa: [ADR-0045](../adr/0045-niebo-atmosfera-hillaire.md).

Prefiltr kosztuje, więc środowisko jest przebudowywane z **debouncingiem** (`SkyIblSettings`):

| Pole | Domyślnie | Znaczenie |
| --- | --- | --- |
| `enabled` | `true` | `false`: nie publikuje `IblSource` (własne środowisko aplikacji zostaje) |
| `image_width` | 128 | szerokość obrazu equirect (parzysta, 16…1024), wysokość to połowa |
| `ibl` | `IblSettings::COMPACT` | rozmiary i próbki prefiltra |
| `min_sun_angle_degrees` | 1,0 | ruch słońca, od którego środowisko się zmienia |
| `min_relative_change` | 0,05 | względna zmiana jasności słońca; bezwzględna zmiana zachmurzenia i albedo |
| `min_altitude_change_meters` | 100 | zmiana wysokości kamery |
| `min_interval_seconds` | 0,5 | najkrótsza przerwa między budowami (`ViewCtx::time_seconds`) |

Pierwsza klatka i zmiana struktury (atmosfera, `image_width`, `ibl`) przebudowują od razu. Zmiana poniżej progów nie robi nic; zmiana powyżej progów w czasie przerwy czeka na jej koniec. Błąd obrazu to `A0002`, bez ponawiania co klatkę.

**3. `SunLight` w `Shared`.**

```rust,ignore
pub struct SunLight {
    pub direction_to_sun: Vec3,        // jednostkowy, od sceny do słońca, świat
    pub illuminance_lux: [f32; 3],     // na powierzchnię zwróconą do słońca, po atmosferze i chmurach
    pub angular_radius: f32,           // promień dysku, rad
}
```

Publikowane co klatkę w `Stage::Setup`, więc cienie i oświetlenie widzą je w tej samej klatce. Iluminancja to `E_atm · T · (1 − zachmurzenie)` na wysokości kamery (całka wprost): białe w południe, czerwone nisko, zero pod horyzontem. To zwykłe dane, bez zależności od `pathon-scene`; `luminous_illuminance()` daje pojedynczą liczbę (luminancja) dla światła z jedną intensywnością.

## Ustawienia

`SkySettings` jest w `Shared` (`SkyPack::with_settings` ustawia początkowe) i jest czytane co klatkę.

| Pole | Domyślnie | Znaczenie |
| --- | --- | --- |
| `sun_direction` | 45° nad horyzontem, azymut 0 (−Z) | świat, Y w górę, od sceny do słońca; `with_sun(wysokość, azymut)` |
| `sun_illuminance_lux` | 120 000 | oświetlenie nad atmosferą, 0…10⁶ |
| `sun_tint` | biały | liniowy mnożnik barwy |
| `sun_angular_radius` | 0,00465 rad | promień dysku, 10⁻⁴…0,05 |
| `draw_sun_disc`, `sun_disc_max_radiance` | `true`, 50 000 | dysk w `hdr` i jego ograniczenie (nity) |
| `cloud_coverage` | 0 | 0…1, patrz niżej |
| `ground_albedo` | 0,3 | albedo ziemi (Lambert) per kanał |
| `camera_height` | `FromCamera { ground_offset_meters: 0 }` | skąd wysokość kamery nad ziemią: `position.y` plus przesunięcie, albo `Fixed { meters }` |
| `atmosphere` | `Atmosphere::EARTH` | media i promienie planety |
| `ibl` | `SkyIblSettings::default()` | tabela wyżej |

Wartości poza zakresem są naprawiane i zgłaszane raz ([A0001](../diag/A0001.md)); kamera poza atmosferą (ponad 100 km albo pod ziemią) jest przycinana ([A0003](../diag/A0003.md)); widok z kosmosu nie jest obsługiwany.

**Zachmurzenie (proste).** `cloud_coverage = c` miesza niebo bezchmurne z jednolitym zachmurzonym (rozkład CIE, `L = L_z (1 + 2 sin h) / 3`) w proporcji `c` i mnoży światło bezpośrednie (dysk, `SunLight`) przez `1 − c`; oświetlenie poziome pod zachmurzonym niebem to ćwierć oświetlenia poziomego z samego słońca (około 25 klx przy słońcu w zenicie). To przybliżenie dnia pochmurnego, nie chmury: bez warstw, kształtów i cieni.

## Moduły i sloty

`pathon-sky` nie wystawia modułów WGSL dla konsumentów: shadery są wewnętrzne. Sloty własne (`Slot::new`, [docs/slots.md](../slots.md)) `TRANSMITTANCE_LUT`, `MULTISCATTER_LUT` i `SKY_VIEW_LUT` (`Rgba16Float`) może czytać każda paczka, np. mgła albo przyszła perspektywa powietrzna; slot, którego nikt nie czyta, znika z grafu razem z przebiegiem, który go pisze.

`pathon_sky::reference` to model na CPU w `f64` (`Planet`, `SkyModel`, `SkyConditions`): wyrocznia testów GPU i źródło obrazu środowiska.

## Diagnostyka

| Kod | Sytuacja |
| --- | --- |
| [A0001](../diag/A0001.md) | ustawienie nieba poza zakresem naprawione |
| [A0002](../diag/A0002.md) | niebo nie zamieniło się w obraz środowiska |
| [A0003](../diag/A0003.md) | wysokość kamery poza atmosferą przycięta |

`SkyFeature` jest wyłączona w negocjacji (`F0101`), gdy `Rgba16Float` nie ma `SAMPLED | FILTERABLE | STORAGE` albo grupa 64 wątków przekracza limit.

## Testy i przykład

- Testy jednostkowe (bez GPU): model CPU (parametryzacja, fazy, horyzont), ustawienia i naprawa, debouncing środowiska (kolejne progi, interwał, cofnięty zegar), składnia i walidacja naga wszystkich modułów WGSL.
- `tests/null_device.rs`: pełny render na urządzeniu null, które sprawdza stany, flagi użycia i bind groupy (z głębią i bez), `SunLight`, liczba przebudowań środowiska, `A0001` i `A0003`.
- `tests/gpu.rs` (feature `gpu-tests`; Vulkan, RX 9070 XT): transmitancja względem całki CPU (zmierzony błąd 37% tolerancji 2% + 3·10⁻⁴), brak NaN i nieskończoności we wszystkich tablicach i w obrazie (słońce od 80° do −8°, zachmurzenie 0…1), zenit niebieski, horyzont o zachodzie czerwieńszy niż w południe, obraz względem modelu CPU (do 13% w najciemniejszym kanale, tolerancja 16%), zachmurzenie, dysk słońca (jest, ograniczony, wyłączalny), rysowanie tylko tam, gdzie głębia jest pusta, irradiancja środowiska zbudowanego z nieba względem całki CPU (do 3,1%, tolerancja 5%) oraz cztery obrazy referencyjne 128×96 w `tests/golden/`: `sky_noon`, `sky_low_sun`, `sky_sunset`, `sky_overcast`. Referencje powstały na Vulkanie; lavapipe, WARP i Metal dopiero w CI ([ADR-0025](../adr/0025-golden-images.md)).
- Przykład `sky`: `cargo run -p pathon-examples --bin sky -- 3 0 0 zachod.png` (wysokość i azymut słońca, zachmurzenie, plik PNG); niebo przez `PostPack` z auto-ekspozycją i bloomem, `IblPack` obok, w logu `SunLight` i środowisko. Opis w [docs/examples.md](../examples.md).

## Nie robi i otwarte

- Perspektywa powietrzna na geometrii (siatka frustum): sloty z tablicami są gotowe, przebiegu nie ma.
- Chmury (warstwy, objętość, cienie), gwiazdy, noc, widok z kosmosu, miękki zachód słońca jako dysku (dziś twardy próg planety).
- Render nieba z GPU wprost do `IblBuilder` (bez obrazu z CPU); dziś obraz z CPU (kilkanaście ms przy zmianie nieba, na wątkach puli, gdy ją mamy).
- Tablice transmitancji i wielokrotnego rozpraszania liczone co klatkę, nie trwałe; brak pomiaru czasu GPU i budżetu na Web.
- Referencje goldena na lavapipe, WARP i Metalu dopiero w CI.
