# ADR-0037: Oświetlenie z otoczenia (IBL): mapa środowiska, prefiltr, irradiancja i LUT BRDF

**Status:** Przyjęta
**Data:** 2026-11-22
**Dotyczy:** M4-06, [ADR-0011](0011-bufory-i-tekstury-trwale-w-grafie.md), [ADR-0020](0020-kontrakty-rhi-types.md) (aneks: cube i `view_formats`), [ADR-0030](0030-material-standardowy-i-hooki-wgsl.md)

## Kontekst

Metal nie ma światła rozproszonego, więc bez otoczenia do odbicia jest czarny; dielektryk bez IBL ma płaski ambient. `pathon-pbr` (M4-05) potrzebuje trzech rzeczy: radiancji odbitej per chropowatość, irradiancji dla Lamberta i tablicy całkowania BRDF (split-sum), a `material::evaluate` (ADR-0030) zwraca `diffuse` i `specular` osobno, więc IBL też musi je rozdzielać. Graf klatki nie opisuje cube'a (`GraphTextureDesc` ma tylko 2D z jednym formatem), a mapa środowiska nie zmienia się co klatkę.

## Decyzja

### Środowisko to trwały zasób poza grafem

- `IblEnvironment` to mały uchwyt `Copy`: widoki i tekstury `irradiance` (cube, 1 mip), `prefiltered` (cube, mipy = chropowatość), `brdf_lut` (2D), sampler, liczba poziomów i `generation`. Graf je tylko importuje (`import_texture(.., Access::SAMPLED, Access::SAMPLED)`, `IblEnvironment::STATE`).
- **Obliczenia nie są węzłem grafu.** Prefiltr jest jednorazowy (raz przy zmianie obrazu), jego wynik nie ma konsumenta w tej samej klatce, a opis cube'a nie mieści się w `GraphTextureDesc`; węzeł z cache'em i historią grafu wymagałby rozszerzenia opisu tekstur przejściowych o coś, czego nikt inny nie potrzebuje. `IblBuilder::build_environment` nagrywa i wysyła jedną listę komend (wszystko w compute), `IblPack` robi to w `record` pierwszej klatki po zmianie `IblSource` w `Shared` (tożsamość `Arc` obrazu i ustawienia) i zwalnia starą mapę przez `release_later` (ADR-0022). Nieudana budowa (np. `I0001`) nie jest ponawiana co klatkę i zostawia poprzednie środowisko.
- Wejście to obraz equirectangular **w pamięci** (`EquirectImage`: f32 RGB/RGBA albo f16 RGBA; przechowywany jako binary16, radiancja nasycana do 65504, NaN i nieskończoność odrzucone jako `I0002`). Rdzeń nie robi I/O: loader Radiance `.hdr` jest w `examples/src/radiance.rs`.

### Potok (compute WGSL, `Rgba16Float`, te same shadery na każdym profilu)

1. **equirect → cube** (`equirect_to_cube`): każdy teksel ściany to średnia siatki podpróbek (do 8×8 na teksel, zależnie od stosunku rozdzielczości), bilinear liczony w shaderze (zawinięcie po długości, zacisk na biegunach). Superpróbkowanie chroni małe jasne słońce przed aliasingiem.
2. **Łańcuch mip** tymczasowej kostki (`downsample_cube`, filtr box, do 1×1): źródło dla *filtered importance sampling*.
3. **Prefiltr GGX** (`prefilter`): mip `m` kostki `prefiltered` to środowisko splecione dla chropowatości percepcyjnej `m / (poziomy − 1)`; mip 0 to ostre środowisko. Założenie split-sum (normalna = widok = odbicie), próbki Hammersleya z rozkładu GGX, waga `n·l`, poziom mipa źródła dobrany do kąta bryłowego próbki (bez fireflies od słońca). Domyślnie 1024 próbki (Desktop), 256 (compact).
4. **Irradiancja** (`irradiance`) i **LUT BRDF** (`brdf_lut`), opis niżej.

Poziomy `prefiltered` kończą się na 8×8 tekseli na ścianę (`log2(cube_size) − 2` poziomów: 6 dla 256, 5 dla 128). Mip 1×1 zapisuje chropowatość 1 w sześciu tekselach, więc kierunek prawie nie wpływa na wynik; pomiar odczytu `ibl_specular` względem całki CPU w 50 kierunkach (sceny z ostrym gradientem ziemia / niebo) dał dla chropowatości 1 błąd maksymalny 16% przy mipie 4×4 i 8% przy 8×8 (średnio 3% i 2%), a mniejsze mipy niczego nie dodają. Chropowatość mapuje się na poziom liniowo (`lod = roughness · (poziomy − 1)`), trójliniowe próbkowanie interpoluje sąsiednie chropowatości (do 8% błędu między poziomami).

### Irradiancja: cubemapa 32², nie SH9

Wybrana jest **cubemapa 32×32** (16×16 w profilu compact) z konwolucją kosinusową policzoną **kwadraturą po każdym teksel mipa 32² źródła** i normalizowaną sumą wag. Powody:

- SH9 drga przy ostrym słońcu HDR (ujemne płaty, błąd dziesiątek procent w cieniu), a poprawki (okno, odszumianie) są heurystyką; cubemapa z kwadraturą nie ma szumu i zachowuje energię słońca, bo mipy to średnie boxowe.
- Furnace jest dokładny: dzielenie przez sumę wag zamiast przez π daje stałe środowisko 1.0 → dokładnie 1.0, niezależnie od błędu kwadratury (`ibl_diffuse` zwraca E/π).
- Ta sama ścieżka bindingów co prefiltr (tekstura + sampler, jedno próbkowanie), bez bufora na współczynniki, bez redukcji (SH wymaga sumy po wszystkich tekselach: atomiki albo dwa przebiegi) i bez czwartego rodzaju zasobu w shaderze konsumenta.
- Koszt jest znany i jednorazowy: 6·32² wyjść × 6·32² odczytów ≈ 38 mln odczytów, 48 KB pamięci. Pomiar względem całki CPU (siatka 256×512): błąd maksymalny 0,7%.

SH9 zostaje jako opcja dla bardzo słabego mobile'a (brak potrzeby dziś); nie zmienia kontraktu `ibl_diffuse(n)`.

### LUT BRDF

2D `Rgba16Float` 128² (64² compact): R = skala `f0`, G = bias `f90`, osie `n·v` (x) i chropowatość percepcyjna (y), teksele na środkach (próbkowanie bez poprawek). `Rg16Float` odpada, bo nie ma `STORAGE` w rdzeniu WebGPU. Całkowanie to to samo co `material::evaluate`: GGX z height-correlated Smith (ta sama funkcja widoczności), 1024 próbki. **Generowany w compute raz** i cache'owany w `IblBuilder` per (rozmiar, liczba próbek); wszystkie środowiska jednego buildera dzielą go (`brdf_lut` jest równe w kolejnych buildach). Weryfikacja: całka CPU po półwektorze (siatka 512×256 strat równego prawdopodobieństwa), błąd maksymalny 0,006.

### Kontrakt WGSL (`package::ibl::sample`, `package::ibl::shading`)

Moduły nie deklarują bindingów (jak ADR-0030), a WGSL pozwala przekazać `texture_cube` i `sampler` jako argumenty funkcji: shader główny deklaruje zasoby sam (`IblBindings::wgsl_declarations(group)` daje tekst, `layout_entries` i `bind_group_entries` resztę) i woła:

| Funkcja | Zwraca |
| --- | --- |
| `ibl_diffuse(irradiance, s, n)` | irradiancja / π: `diffuse_color · ibl_diffuse` to wyjściowa radiancja Lamberta |
| `ibl_specular(prefiltered, s, r, roughness)` | radiancja odbita, `lod = roughness · (poziomy − 1)` |
| `ibl_brdf(lut, s, n_dot_v, roughness)` | `(skala, bias)` |
| `ibl_split_sum(f0, f90, brdf)` | `f0 · skala + f90 · bias` |
| `ibl_evaluate(.., IblInput) -> IblResult` | `diffuse` i `specular` **osobno**, jak `BsdfResult` z `material::evaluate` |

`IblInput` niesie to, co daje `Surface` z `load_surface`: `normal`, `view`, `diffuse_color`, `f0`, `f90`, `roughness`. `ibl_evaluate` bierze odbicie w kierunku dominującym (Lagarde), mnoży specular przez `f0·skala + f90·bias` oraz kompensację wielokrotnego rozpraszania (Fdez-Agüera; bez niej szorstkie metale tracą do jednej trzeciej energii), a diffuse przez `1 − albedo spekularne`. Test biały furnace (środowisko 1.0): diffuse albedo 1 daje 1 ±2%, biały metal 1 ±3% dla chropowatości 0,05…1 i `n·v` 0,2…1, dielektryk (`f0 = 0,04`) w 0,98…1,06.

**Poza zakresem** (należą do `pathon-pbr`, M4-05, albo do późniejszych zadań): okluzja (`surface.occlusion`, AO) i okluzja spekularna, horizon fade, clearcoat jako druga próbka `ibl_specular`, waga transmisji i SSS, kolejność z cieniami i światłami punktowymi, niebo jako źródło środowiska (M4-08).

### Diagnostyki `I`

`I0001` złe ustawienia, `I0002` zły obraz, `I0003` urządzenie bez `Rgba16Float` `STORAGE | FILTERABLE` (wtedy `IblPack` jest wyłączony w negocjacji), `I0004` obraz większy niż limit tekstury.

## Konsekwencje

- `pathon-pbr` nie zależy od tego crate'u w czasie kompilacji: czyta `IblEnvironment` z `Shared`, a WGSL składa z `pathon_ibl::modules()` (pack-boundary bez zmian).
- Pamięć środowiska 256: prefiltr `6 · 256² · 8 B · 1,33 ≈ 4,2 MB` plus irradiancja 48 KB plus LUT 128 KB; tymczasowa kostka źródłowa i obraz equirect są niszczone zaraz po wysłaniu komend.
- Nasycenie 65504 obcina energię bardzo jasnych słońc z plików HDR (zwykle 1e5 i więcej); mipy pozostają skończone. Użytkownik, któremu to przeszkadza, dodaje słońce jako światło kierunkowe.
- Tolerancje testów liczbowych (prefiltr 6% + 0,02 względem całki CPU, irradiancja 3% + 0,01, LUT 0,01) są dobrane do próbkowania z filtrem i `f16` w pamięci; zmierzone błędy maksymalne to 3,4% / 2,7% / 2,0% / 5,2% / 7,7% (mipy 0, 1, 2, 4, 5 kostki 256, błąd względem `oczekiwane + 0,05`), 0,7% (irradiancja) i 0,006 (LUT).
- Otwarte: referencja goldena `ibl_metals` na lavapipe, WARP i Metalu dopiero w CI (pierwszy przebieg może wymagać referencji per backend, ADR-0025); SH9 dla słabego mobile'a; okluzja spekularna i clearcoat w IBL; środowisko z nieba proceduralnego (M4-08) tą samą ścieżką (`EquirectImage::from_fn` albo kostka źródłowa wprost).
