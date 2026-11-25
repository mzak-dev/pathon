# ADR-0045: Niebo: atmosfera Hillaire 2020 jako tło, słońce i źródło IBL

**Status:** Przyjęta
**Data:** 2026-11-25
**Dotyczy:** M4-08, [ADR-0034](0034-wyjscie-hdr.md), [ADR-0037](0037-ibl-srodowisko-prefiltr-i-lut.md), [ADR-0041](0041-pack-lib.md), [ADR-0047](0047-uzywanie-kodu-filamenta.md)

## Kontekst

Scena potrzebuje trzech rzeczy od nieba: tła w slocie `hdr` tam, gdzie nic nie narysowano, światła do odbić i ambientu (IBL, [ADR-0037](0037-ibl-srodowisko-prefiltr-i-lut.md)) oraz słońca jako światła kierunkowego o barwie zgodnej z porą dnia. Wszystkie trzy muszą wynikać z jednego modelu, inaczej słońce nisko nad horyzontem oświetla scenę na biało pod czerwonym niebem. Wymagania: tylko compute i render, bez atomików 64-bitowych (Web), jednostki fizyczne (lux, nity; [docs/slots.md](../slots.md)), bez zależności od `pathon-scene`.

Implementacja powstaje od zera z publikacji (Hillaire, *A Scalable and Production Ready Sky and Atmosphere Rendering Technique*, EGSR 2020; Bruneton i Neyret 2008 dla parametryzacji transmitancji), zgodnie z [ADR-0047](0047-uzywanie-kodu-filamenta.md): bez cudzego kodu.

## Decyzja

### Model i tablice (WGSL, w grafie klatki)

Rayleigh, Mie (faza Cornette'a-Shanksa, `g = 0,8`) i absorpcja ozonu, jednostki: kilometry i współczynniki na kilometr (Ziemia: promienie 6360 i 6460 km). Cztery przebiegi w `Stage::Sky`, każdy z własnym modułem WGSL (konkatenacja plików, bo dwa przebiegi deklarują różne zasoby pod tym samym bindingiem):

1. **Transmitancja** 256×64: tablica w parametryzacji Bruneton-Neyret `(μ, r)`, 40 kroków całkowania.
2. **Wielokrotne rozpraszanie** 32×32: `Ψ_ms` z paczki (64 kierunki, 20 kroków na kierunek, rozpraszanie drugiego rzędu z fazą izotropową, szereg geometryczny `1 / (1 − f_ms)`), z odbiciem od ziemi w pierwszym rzędzie.
3. **Sky-view** 192×108: luminancja nieba na jednostkę oświetlenia słońca w parametryzacji z horyzontem w połowie wysokości i gęstością przy słońcu; marsz 32 kroków rozmieszczonych kwadratowo (gęsto przy kamerze) ze źródłem = słońce × faza + `Ψ_ms × σ_s`.
4. **Rysowanie**: trójkąt pełnoekranowy czyta sky-view, transmitancję i (gdy jest) głębię i pisze do `hdr`.

Wszystkie tablice to `Rgba16Float` (zapis z compute, filtrowanie; WebGPU ma to w rdzeniu), przebiegi to zwykłe grupy 8×8 bez pamięci współdzielonej i atomików. Rozmiary i kroki to `SkyQuality` (`DESKTOP`, `COMPACT` dla Web i mobile), stałe po zbudowaniu rendera, bo wchodzą w pipeline'y i tekstury.

**Tablice liczone co klatkę, nie trwałe.** Transmitancja i wielokrotne rozpraszanie zależą tylko od atmosfery i albedo ziemi, więc dałoby się je trzymać między klatkami. Zostają w grafie, bo to oszczędza stan (unieważnienie przy zmianie ustawień, utrata urządzenia, zwalnianie), a koszt jest rzędu ułamka milisekundy na desktopie (dla Web wariant `COMPACT`; pomiaru czasu GPU jeszcze nie ma). Zmiana ustawień nieba działa natychmiast bez żadnej obsługi. Tablice są własnymi slotami (`sky.transmittance`, `sky.multiscatter`, `sky.sky_view`), więc inne paczki (mgła, perspektywa powietrzna) mogą je czytać; slot, którego nikt nie czyta, znika z grafu razem z producentem.

**Precyzja f32.** Kamera jest kilka metrów nad promieniem 6360 km, a `r² − R²` w f32 traci cyfry. Wszędzie, gdzie się go używa, jest iloczyn `(r − R)(r + R)`, a wysokość punktu marszu to `(ρ² + 2 r μ t + t²) / (|P| + R)` zamiast `|P| − R`.

### Tło: głębia i slot `hdr`

Przebieg działa po geometrii (`Stage::Sky`), ładuje `hdr` i pisze tam, gdzie głębia jest równa zeru (reverse-Z, `Slot::DEPTH`); bez slotu głębi rysuje wszędzie, a bez slotu `hdr` sam go tworzy i czyści. Kierunek piksela to różnica dwóch odwróconych punktów promienia (głębia 1 i 0,5), więc działa dla każdej projekcji reverse-Z, także z nieskończoną płaszczyzną dalszą. Dysk słońca ma promień kątowy z ustawień, miękką krawędź (od 0,8 promienia) i jest dimmowany transmitancją; jego radiancja (≈ 1,8·10⁹ nitów) nie mieści się w binary16, więc jest ograniczona do `sun_disc_max_radiance` (domyślnie 50 000 nitów). Ten sam limit stosuje `EquirectImage` (65 504), dlatego **dysku nie ma w środowisku IBL**: słońce w oświetleniu to `SunLight`.

### Źródło IBL: obraz równoprostokątny z CPU, z debouncingiem

`IblSource` ([ADR-0037](0037-ibl-srodowisko-prefiltr-i-lut.md)) niesie `EquirectImage`, czyli obraz **w pamięci CPU**, a odczyt tekstury GPU w `record` nie jest przenośny (na Web nie ma synchronicznego odczytu). Zamiast niego `pathon-sky` liczy niebo na CPU tym samym modelem co shadery (`pathon_sky::reference`, f64): tablice transmitancji (256×64) i wielokrotnego rozpraszania (16×16, 8×8 kierunków) budowane raz na atmosferę i albedo, potem marsz 24 kroków na piksel obrazu 128×64 (domyślnie), z wierszami rozdzielonymi przez `TaskPool::parallel_for`. Obraz trafia do `Shared` jako `IblSource`, a `IblPack` robi resztę bez zmian (`pathon-ibl` nie dostał nowego API).

Koszt prefiltra pilnuje **debouncing** (`SkyIblSettings`): nowy obraz powstaje przy pierwszej klatce, od razu po zmianie struktury (atmosfera, rozmiar obrazu, ustawienia IBL) i po zmianie powyżej progu (słońce o ≥ 1°, jasność słońca o ≥ 5%, zachmurzenie lub albedo o ≥ 0,05, wysokość kamery o ≥ 100 m), ale nie częściej niż co `min_interval_seconds` (0,5 s, zegar `ViewCtx::time_seconds`). Zmiana poniżej progu nie przebudowuje niczego, zmiana powyżej progu w czasie przerwy czeka na jej koniec. Cecha `SkyEnvironmentFeature` ma `before = ["ibl"]`, więc oświetlenie nie spóźnia się o klatkę za niebem.

Alternatywa, odrzucona na razie: render nieba z GPU do cube'a i wejście do `IblBuilder` wprost z tekstury (bez CPU). Wymaga nowego wejścia w `pathon-ibl` (`IblSource` z teksturą zamiast obrazu) i jest właściwą drogą, gdy czas obrazu z CPU (rzędu kilkunastu ms w jednym wątku, przy zmianie nieba) zacznie przeszkadzać; kontrakt `SunLight` i `IblSource` się wtedy nie zmienia.

### Słońce: `SunLight` w `Shared`, `pathon-sky` jako `pack-lib`

Każda klatka (w `Stage::Setup`, przed cieniami i oświetleniem) publikuje `SunLight { direction_to_sun, illuminance_lux: [f32; 3], angular_radius }`: oświetlenie powierzchni zwróconej do słońca na wysokości kamery, czyli `E_atm · T(r, μ_słońca) · (1 − zachmurzenie)`, całkowane wprost (nie z tablicy), zero, gdy planeta zasłania słońce. To zwykłe dane: nie ma zależności od `pathon-scene`.

Żeby `pathon-pbr` i `pathon-shadow` mogły odczytać `SunLight` z `Shared` po typie, typ musi być widoczny dla innych paczek, więc crate ma `kind = "pack-lib"` ([ADR-0041](0041-pack-lib.md)): zależy tylko od `pathon-api` i `pathon-ibl` (też `pack-lib`), rdzeń o nim nie wie, baseline domknięcia rdzenia się nie zmienia. Alternatywą byłby typ w `pathon-scene` (zakazane przez zadanie) albo w `pathon-api` (rdzeń dostaje pojęcie słońca, czego zasada „rdzeń nie zna technik” nie lubi).

### Zachmurzenie: tylko proste

`cloud_coverage ∈ [0, 1]` miesza niebo bezchmurne z niebem zachmurzonym w całości i odbiera światło bezpośrednie: dysk i `SunLight` mnożone przez `1 − c`. Niebo zachmurzone to rozkład CIE (`L = L_z (1 + 2 sin h) / 3`) z luminancją zenitu dobraną tak, by oświetlenie poziome było ćwiartką oświetlenia poziomego z samego słońca (`E_h = 0,25 · E_atm · T · max(sin h_słońca, 0)`, `L_z = 9 E_h / 7π`; ok. 25 klx przy słońcu w zenicie, zgodnie z pochmurnym dniem). To przybliżenie, nie model chmur: bez warstw, cieni chmur ani objętości.

### Wysokość kamery

`CameraHeight::FromCamera { ground_offset_meters }` (domyślnie: `Camera::position.y` jest wysokością nad ziemią) albo `Fixed { meters }`; wynik jest przycinany do atmosfery (A0003). Widok z kosmosu nie jest obsługiwany.

### Diagnostyki `A`

`A0001` ustawienie naprawione (jak `Q0001`: raz, do czasu poprawy), `A0002` obraz środowiska odrzucony przez `EquirectImage` (błąd paczki), `A0003` wysokość kamery poza atmosferą.

## Konsekwencje

- Jedno źródło prawdy dla tła, ambientu i słońca: pora dnia zmienia barwę wszystkich trzech naraz.
- Test GPU na RX 9070 XT (Vulkan): tablica transmitancji względem całki CPU (256 kroków) w granicach 37% tolerancji (2% + 3·10⁻⁴), obraz względem modelu CPU z błędem do 13% w najciemniejszym kanale (słońce 2° nad horyzontem, patrzenie od słońca; tolerancja 16%), irradiancja środowiska zbudowanego z nieba względem całki kosinusowej CPU w 6 kierunkach i dwóch porach dnia: do 3,1% (tolerancja 5%). Referencje goldenów dla lavapipe, WARP i Metala dopiero w CI ([ADR-0025](0025-golden-images.md)).
- Obraz środowiska z CPU nie ma dysku słońca i jest liczony w jednym wątku na platformach bez puli (Web): kilkanaście do kilkudziesięciu milisekund przy zmianie nieba, ograniczone debouncingiem.
- Wysokość kamery w środowisku IBL jest jedna na rendera; przy kilku widokach na różnych wysokościach środowisko pochodzi z ostatnio nagranego (próg 100 m i przerwa ograniczają przełączanie).

## Otwarte

- Perspektywa powietrzna (siatka frustum 32×32×32 z transmitancją i rozpraszaniem na głębię): kontrakt slotów jest gotowy (tablice są slotami), przebieg nie powstał.
- Cienie planety w pobliżu terminatora (słońce jako dysk, nie punkt: dziś twardy próg), chmury, gwiazdy i noc, widok z kosmosu.
- Wejście `IblBuilder` z tekstury (render nieba z GPU bez obrazu CPU).
- Pomiar czasu GPU przebiegów i ich kosztu na Web; trwałe tablice transmitancji i wielokrotnego rozpraszania, jeśli pomiar to uzasadni.
