# ADR-0049: Globalne oświetlenie z sond irradiancji i wokseli (`pathon-gi`)

**Status:** Przyjęta
**Data:** 2026-11-26
**Dotyczy:** K12 ([filament-reference.md](../filament-reference.md)), [ADR-0041](0041-pack-lib.md) (`pack-lib`), [ADR-0042](0042-pbr-prepass-klastry-i-forward.md) (forward), [ADR-0037](0037-ibl-srodowisko-prefiltr-i-lut.md) (IBL), [ADR-0047](0047-uzywanie-kodu-filamenta.md) (bez kodu Filamenta), [ADR-0048](0048-swiatla-powierzchniowe-prostokatne.md), [docs/porownanie-obrazu.md](../porownanie-obrazu.md), kody `J0001`…`J0006`

## Kontekst

Pomiar K01 pokazał, że oświetlenie pośrednie to 55 do 64% luminancji classroom, a około 70% tego światła wchodzi przez okna; doklejenie oświetlenia pośredniego wzorca do pathon podnosiło SSIM z 0,67 do 0,83. Drugie źródło błędu to wyciek IBL: środowisko nie wie o ścianach, więc wnętrze dostawało całe niebo (+25 do +51% luminancji w wariancie `ibl`, w Cyclesie +0,5 do +2,5%). `wgpu` 30 nie ma śledzenia promieni na Web i Metalu, więc rozwiązanie musi stać na rasteryzacji i compute. Szkic zakładał sondy z kubemapami; wybór i uzasadnienie liczbami są tutaj.

Wzory pochodzą z publikacji (ADR-0047): Majercik, Guertin, Nowrouzezahrai, McGuire 2019 („Dynamic Diffuse Global Illumination with Ray-Traced Irradiance Fields”), Majercik i in. 2021 („Scaling Probe-Based Real-Time Dynamic Global Illumination for Production”), Amanatides i Woo 1987 (DDA), Akenine-Möller 2001 (trójkąt-sześcian), Cigolle i in. 2014 (oktaedr), Keinert i in. 2015 (spiralna Fibonacciego).

## Rozważone drogi

**(a) Sondy z kubemapami rysowanymi przez istniejące passy** (każda sonda co N klatek rysuje scenę do małej kostki). Odrzucona. `PbrPack` nie ma culling (jedno rysowanie na instancję, [ADR-0042](0042-pbr-prepass-klastry-i-forward.md)); classroom ma 862 instancje, prepass i kolor dają ok. 10 tys. rysowań na sondę (6 ścian × 2 passy × 862), a objętość ma 3168 sond: pełne odświeżenie to ok. 33 mln rysowań. Przy zmierzonym koszcie nagrywania pbr (1,5 ms CPU na 2 × 862 rysowań, ok. 0,9 µs) każde 8 sond na klatkę kosztowałoby ok. 75 ms CPU (oszacowanie, nie pomiar). Paczka też nie może dodać widoków do klatki, a `pack-boundary` zabrania jej zależeć od `pathon-pbr`, więc sondy musiałby karmić host. Na GPU rasteryzacja drobnych kostek ma małą wydajność (kilka tysięcy trójkątów na piksel).

**(b) SSGI** (promienie w przestrzeni ekranu). Nie jest tym, czego brakuje: światło wchodzące przez okno pochodzi z tego, czego kamera nie widzi (lampa za oknem, niebo), a ekran nie niesie widoczności nieba dla wnętrza. Zostaje jako uzupełnienie w K13, nie w tym zadaniu.

**(c) Sondy irradiancji z śledzeniem w siatce wokseli (wybrana).** Scena jest raz redukowana do siatki wokseli (albedo, emisja, światło odbite z sześciu ścian komórki), a sondy śledzą promienie przez tę siatkę w compute. To DDGI bez sprzętowego RT: zamiast BVH jest DDA po siatce, zamiast shadera trafienia jest odczyt oświetlonej ściany wokselu. Działa na każdym backendzie z compute (Web włącznie), nie wymaga zmiany `pathon-core` ani `PbrPack` poza jedną permutacją.

## Decyzja

### 1. Zależności (poprawka do ADR-0041)

`pathon-gi` ma `kind = "pack-lib"` (jak `pathon-shadow` i `pathon-sky`): `PbrPack` musi czytać `GiFrame` i moduł WGSL, a paczka nie może zależeć od paczki. `GiPack` pozostaje zwykłą paczką, którą dodaje aplikacja. Zależności `pathon-gi`: `pathon-api`, `pathon-shader` i `pack-lib` `pathon-mesh`, `pathon-scene`, `pathon-material`, `pathon-bindless`, `pathon-ibl`. `xtask deps-check` przechodzi, domknięcie rdzenia bez zmian (126 crate'ów).

### 2. Dane

- **Siatka wokseli** (domyślnie 0,1 m, do 3 mln komórek, rośnie z rozmiarem sceny): 9 słów na komórkę (albedo `sqrt` w bajtach plus bit zajętości, emisja w RGB ze wspólnym wykładnikiem, radiancja sześciu ścian w tym formacie, miejsce na liście komórek stałych). Radiancja jest **per ściana komórki** (anizotropowa), a promień czyta ścianę, przez którą wszedł do komórki: dwa pomieszczenia za cienką ścianą nie mieszają swojego światła.
- **Sondy:** regularna siatka (0,5 m, do 4096) wyśrodkowana na pudełku sceny. Atlas irradiancji `Rgba16Float` (oktaedr 8×8 plus ramka, rgb to kosinusowo ważona średnia radiancji E/π, alfa to widoczność nieba) i atlas momentów odległości `Rgba16Float` (oktaedr 16×16 plus ramka, średnia i średnia kwadratów, odległość znormalizowana do `max_ray_distance`). Dwie generacje (czytana i pisana), trwałe poza grafem, importowane do grafu w stanie `SAMPLED`.
- Konwencja E/π zamiast E: ta sama co `ibl_diffuse`, więc `pbr` mnoży przez albedo bez π.

### 3. Klatka

Cztery passy compute w `Stage::Setup` z priorytetem przed „clear output” (kolejność jawna, jak `skin`): woksalizacja (tylko po zmianie sceny), oświetlenie komórek, promienie, aktualizacja atlasów.

- **Woksalizacja:** konserwatywna (pełny test 13 osi), więc powierzchnia nie ma dziur, przez które DDA mógłby przejść (DDA przechodzi tylko przez ściany komórek). Cena: pełne ściany wychodzą do jednego wokselu za grube, więc pokój jest nieco mniejszy (zmierzony bias +8,3% w Cornellu 4 m przy wokselu 10 cm). Albedo z tekstury bazowej próbkowanej gradientem wielkości komórki (to samo `sample_material_texture`, co woła `pbr`, więc bindless i bind group na materiał działają jednakowo). Materiały z blendem i transmisją powyżej 0,5 nie zatrzymują promieni.
- **Oświetlenie komórek:** wątek na ścianę komórki z listy komórek stałych (lista budowana po woksalizacji atomikiem 32-bitowym); światła w tych samych jednostkach i wzorach co `pbr::lights`, cień z samych wokseli, światło obszarowe jako siatka emiterów kosinusowych (co 0,7 m, ≤ 12×12), plus światło sond z poprzedniej klatki (kolejne odbicia: energia zbiega, bo albedo < 1) i emisja. Jedna czwarta komórek na klatkę (przez 8 pierwszych klatek wszystkie), co daje histerezę czasową także zmianom świateł.
- **Promienie:** 128 na sondę, spiralna Fibonacciego obracana losowo co klatkę; środowisko dla promieni, które opuszczają siatkę: prefiltrowany sześcian IBL, mip 0,3 ostatniego poziomu (bez wariancji pojedynczego tekselka słońca). Sonda w komórce stałej jest wyłączona (zera w obu atlasach, waga 0 w próbkowaniu).
- **Aktualizacja:** histereza 0,95 (pierwsze 8 klatek rośnie od 0), ważone kosinusem (irradiancja) i `cos^12` (odległość); ramki atlasu liczone tym samym kodem przez odwzorowanie oktaedralnej krawędzi, więc próbkowanie bilinearne nie wychodzi z kafla.

### 4. Próbkowanie i `PbrPack`

`gi::sample::gi_sample` (WGSL, moduł bez bindingów jak `ibl::sample`): osiem sond, trójliniowo, z wagą `(0,5 + 0,5·cos)² + 0,2` od tyłu sondy i testem Czebyszewa na momentach (waga razy sześcian `σ² / (σ² + Δ²)`); przesunięcie punktu zapytania o `normal_bias` wzdłuż normalnej i `view_bias` ku oku; `confidence` gaśnie na krawędzi objętości i tam, gdzie żadna sonda nie widzi punktu. `PbrPack` dostał flagę permutacji `gi` i cztery bindingi (`GI_FIRST_BINDING`, po tablicy LTC); bez `GiFrame` permutacja i obraz są takie jak przed K12. Diffuse z sond **zastępuje** diffuse IBL (z wagą `confidence`), a specular IBL jest zaciemniane widocznością nieba z sond (`specular_occlusion`).

### 5. Cechy bez zmian rdzenia

Wejścia czytane z `Shared` jak w `PbrPack` (`Snapshot`, `GeometryPool`, `MaterialTable`, opcjonalnie `TextureBinder`, `MaterialHooks`, `IblEnvironment`); skala nieba to `GiSettings::environment_intensity` (host ustawia jak `PbrSettings`, viewer robi to sam). Cecha ma `after = ["ibl"]`.

## Pomiary (RX 9070 XT, Vulkan, classroom 1920×1080, `--bench 120`, najlepsze z 4 uruchomień)

| | GPU razem | `gi light voxels` | `gi trace probes` | `gi update probes` | `pbr forward` | CPU record+submit |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| bez GI | 1,65 ms | | | | 1,20 ms | 1,5 ms |
| `--gi` (woksel 10 cm, okres 4) | 3,83 ms | 1,00 ms | 0,22 ms | 0,48 ms | 1,67 ms | 2,6 ms |
| `--gi --gi-voxel 0.05 --gi-period 8` | 4,09 ms | 2,03 ms | 0,33 ms | 0,32 ms | 1,09 ms | 2,9 ms |

Objętość: 3168 sond (0,5 m), 405 504 promieni na klatkę, 307 632 wokseli po 10 cm (43,5 MiB) albo 2,3 mln po 5 cm (112 MiB). Pierwsza wersja oświetlenia komórek (wątek na komórkę z pętlą po ścianach i światłach) kosztowała 10 do 14 ms niezależnie od okresu: bez równoległości DDA jest ograniczone opóźnieniem pamięci; lista komórek stałych i wątek na ścianę dały 10× mniej.

Jakość względem Cyclesa (K01, `compare_all.py`, średnia z trzech kamer, wzorzec `reference` ze światłami obszarowymi, `lights-only`; pathon po K05):

| | PSNR dB | SSIM | ΔE2000 śr. | ΔE2000 p95 | bias | bias bez okien |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| przed (bez GI) | 13,5 | 0,652 | 11,3 | 44,2 | 0,55 | 0,84 |
| GI, woksel 10 cm | 15,4 | 0,673 | 10,0 | 31,9 | 0,77 | 1,04 |
| GI, woksel 5 cm | 16,5 | 0,701 | 8,9 | 26,6 | 0,78 | 1,01 |

Wyciek IBL (wariant `ibl` względem `lights-only`, `bias`): przed 0,70 wobec 0,55 (+27%), po 0,79 wobec 0,78 (+1%); podłoga w `ibl` wobec `lights-only`: przed 1,57 wobec 0,97, po 1,11 wobec 1,00. Pełne tabele w [porownanie-obrazu.md](../porownanie-obrazu.md#k12-sondy-irradiancji).

Testy fizyczne: biały piec (pokój z emisją 1 i albedo 0,5, oczekiwane 2,0): średnia 1,975, najgorsza sonda 1,5%; Cornell względem Monte Carlo na CPU: bias +8,3%, najgorszy kanał 12,6%; ciemny pokój 0,4 m za jasnym: 3·10⁻⁴ przy 1,96; zmiana irradiancji po 30 klatkach statycznej sceny: średnio 0,6%, najgorsza sonda 2,7%.

## Konsekwencje

- Najwięcej oświetlenia pośredniego wraca, ale **okna nadal są ciemniejsze niż we wzorcu** (luminancja w masce `windows` 0,63 / 0,44 / 0,39 wzorca wobec 0,14 / 0,17 / 0,16 przed GI): żaluzje i szprosy cieńsze niż woksel stają się pełnymi ścianami; prawdopodobna, niezbadana osobno przyczyna to też ich oświetlenie bezpośrednie i szkło bez załamania. SSIM 0,70 i ΔE 8,9 zostają w żółtym paśmie progów K01 (zielone: SSIM ≥ 0,80, ΔE ≤ 7,5); `bias_total` 0,78 wymaga okien.
- Koszt: +2,2 ms GPU i +1,1 ms CPU przy 1080p w domyślnej konfiguracji, pamięć 43,5 MiB; pełna ponowna woksalizacja przy każdej zmianie sceny (obiekty ruchome: wymagana aktualizacja przyrostowa, otwarte), objętość nie podąża za kamerą (otwarte dla dużych scen), skórka i morfy w pozie bazowej.
- Artefakty widoczne na Cornellu: plamy o rozmiarze odstępu sond (0,5 m) na gładkich ścianach i ciemny pasek przy górnej krawędzi niższego pudła (woksel grubszy niż warstwa, na której leży sonda).
- Koszt CPU 1,1 ms/klatkę wynika ze zbierania sceny co klatkę; do wyniesienia za licznik epoki `Snapshot`.
- Referencja goldena (`tests/golden/gi_cornell.png`) powstała na Vulkanie RX 9070 XT; lavapipe, WARP i Metal dopiero w CI.
