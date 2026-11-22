# ADR-0030: Materiał standardowy jako funkcja WGSL i hooki paczek

**Status:** Przyjęta
**Data:** 2026-11-22

## Kontekst

Forward+ (M4-05), visibility buffer (M8-02) i punkt trafienia ray tracingu (M8-03) shadują ten sam materiał w trzech różnych shaderach: fragment, compute po visibility bufferze i hit shader. Gdyby materiał był kodem entry pointu, każdy z nich miałby własną kopię modelu BSDF, a wyniki by się rozjechały. Bindless (M4-04) i `pathon-scene` (M4-02) potrzebują z kolei stabilnego **logicznego indeksu materiału** i stałego układu danych, zanim powstanie kod, który je zakłada. Hook materiału z M2-14 był atrapą (sloty na żądanie); teraz paczka ma móc zmienić materiał w WGSL.

## Decyzja

### Materiał to funkcje WGSL w modułach wesl, nie entry point

Crate `pathon-material` (paczka, zależy od `pathon-api` i `pathon-shader`) dostarcza moduły WESL ([ADR-0021](0021-importy-shaderow-wesl.md)):

| Moduł | Zawartość |
| --- | --- |
| `package::material::surface` | `SurfaceInput` (wejście z entry pointu), `Surface` (rozwiązane parametry), `LightSample`, `BsdfResult` |
| `package::material::standard` | `MaterialData`, `load_surface`, `evaluate`, `transmission_weight`, `volume_transmittance`, `passes_alpha_test`, `output_alpha` |
| `package::material::hooks` | `modify_surface(input, surface) -> Surface` (domyślnie tożsamość) |
| `package::material::texture_source` | `sample_material_texture(slot, uv, uv_dx, uv_dy) -> vec4` (domyślnie biel) |

- **Entry point należy do wołającego.** Moduły nie deklarują żadnego bindingu: `var<storage, read> materials: array<MaterialData>` stoi w shaderze głównym, który sam wybiera grupę i numer. Dzięki temu ta sama funkcja działa w fragmencie, compute i hit shaderze oraz z `binding_array` albo bind group per materiał (M4-04).
- **Bez pochodnych ekranowych.** `SurfaceInput` niesie jawne gradienty UV (`uv_dx`, `uv_dy`); materiał nigdy nie woła `dpdx`/`dpdy` ani `textureSample`. Fragment liczy gradienty sam (`dpdx(uv)`), compute po visibility bufferze liczy je analitycznie z trójkąta, hit shader z kamery lub z konów promieni. Próbkowanie tekstur idzie przez `textureSampleGrad` w module `texture_source`, który nie wymaga jednolitego przepływu sterowania.
- `evaluate(surface, view, light) -> BsdfResult { diffuse, specular }` zwraca **składowe osobno**. To hook podziału `diffuse` / `specular`: okluzja i AO działają na diffuse, odszumianie i ReSTIR (M8-03) rozdzielają oba, a IBL (M4-06) bierze `Surface::diffuse_color`, `f0`, `f90` i `roughness`. `light.direction` i `view` to jednostkowe wektory od powierzchni; wynik to promieniowanie (BRDF · radiancja · cos θ), nie sama BRDF.
- Światło jest pojedynczą próbką (`LightSample`). Pętla po światłach, cienie i IBL należą do `pathon-pbr`, nie do materiału.

### Hooki paczek: podmiana modułu

WESL nie ma funkcji wirtualnych, więc hook to **moduł, który paczka podmienia** pod tą samą ścieżką (`ShaderLibrary::add_module` / `BackgroundCompiler::add_module` zastępują poprzedni; ADR-0023). `MaterialHooks` (typ współdzielony przez `Shared`) trzyma podmienione źródła, a `modules()` oddaje komplet czterech modułów do rejestracji. Hook musi zachować sygnaturę domyślnego modułu. `MaterialHooks::validate` składa moduły bez flag i ze wszystkimi flagami naraz i waliduje je w naga, więc zły hook daje `Y0003` przy rejestracji, a nie przy pierwszym rysowaniu. Dwa hooki (`modify_surface` po załadowaniu powierzchni, `sample_material_texture` na każdy odczyt tekstury) wystarczają dla M4-03: pierwszy pozwala paczce zmienić dowolny parametr (np. mokra powierzchnia z deszczu zmienia `roughness`), drugi podpina bindless w M4-04 bez dotykania materiału. Tylko jeden hook danego typu naraz: dwie paczki, które chcą `modify_surface`, muszą się porozumieć przez własną paczkę kompozytową (rdzeń nie składa hooków).

### Model: glTF 2.0 metallic-roughness + podzbiór KHR

Wybór to część wspólna `StandardMaterial` z Bevy i `StandardMaterial3D` z Godota oraz tego, co glTF ma jako rozszerzenia KHR, dzięki czemu adaptery hostów (M6-02, M7-05) mapują pola 1:1:

- rdzeń glTF: `base_color`, `metallic`, `roughness`, `normal_scale`, `occlusion_strength`, `emissive`, `alpha_mode` (opaque / mask / blend), `double_sided`, tekstury (podstawowy kolor, metallic-roughness w układzie glTF G/B, normalna, okluzja, emisja; jeden zestaw UV);
- `KHR_materials_emissive_strength`, `KHR_materials_ior`, `KHR_materials_specular` (współczynnik i kolor), `KHR_materials_clearcoat` (współczynnik i chropowatość, bez osobnej mapy normalnych), `KHR_materials_transmission`, `KHR_materials_volume` (grubość, kolor i dystans tłumienia), `KHR_materials_diffuse_transmission` (flaga `subsurface`).

Model: Lambert (z mnożnikiem `1 − F`, jak w dodatku B glTF) albo Burley (Disney, `DiffuseModel::Burley`), GGX z height-correlated Smith i Schlick Fresnel; clearcoat to druga warstwa GGX z `F0 = 0,04`, której Fresnel tłumi warstwę bazową (przybliżenie Filamenta, `VdotH` zamiast `NdotV`).

**Poza zakresem M4-03** (miejsce w strukturze jest do rozszerzenia przez dopisanie pól na końcu `MaterialData`): sheen (poza częścią wspólną obu silników: `StandardMaterial3D` ma „rim”, czyli inny model), anisotropy (jest w obu silnikach, ale wymaga tangentów w `evaluate` i osobnego lobe), iridescence, dispersion, mapa normalnych clearcoatu, `KHR_texture_transform`, drugi zestaw UV, parallax i detail maps z Godota. Prawdziwe SSS (rozmycie w przestrzeni ekranu) i załamanie tła należą do `pathon-pbr` / `pathon-post`: materiał oddaje tylko wagę (`transmission_weight`) i transmitancję objętości (`volume_transmittance`), a wołający mnoży je przez to, co leży za powierzchnią.

### Flagi permutacji

Permutacja to flagi WESL `@if(...)` ([ADR-0023](0023-permutacje-shaderow.md)); `MaterialVariant` mapuje opis materiału na `ShaderKey`:

| Flaga | Włączana gdy | Efekt w shaderze |
| --- | --- | --- |
| (brak) | `AlphaMode::Opaque` | `output_alpha` = 1, `passes_alpha_test` = true |
| `masked` | `AlphaMode::Mask` | `passes_alpha_test`: `alpha >= alpha_cutoff` (`discard` robi entry point) |
| `transparent` | `AlphaMode::Blend` | `output_alpha` = `alpha` |
| `subsurface` | `diffuse_transmission > 0` | lobe transmisji dyfuzyjnej, tylna strona oświetlona w `diffuse` |
| `transmission` | `transmission > 0` | `diffuse` skalowane przez `1 − transmission`, wypełnia grubość i tłumienie objętości |
| `clearcoat` | `clearcoat > 0` | druga warstwa GGX |
| `burley` | `DiffuseModel::Burley` | dyfuzja Burleya zamiast Lamberta |

`masked` i `transparent` wykluczają się (jeden `AlphaKind`), więc jest 3 · 2⁴ = 48 wariantów; „opaque” z planu to brak obu flag, nie osobna flaga, żeby nie dało się włączyć sprzecznych. Wartości liczbowe nie są flagami: idą w danych materiału. Pipeline zbudowany dla wariantu `V` rysuje poprawnie materiał o wariancie `M` tylko gdy `V.covers(M)`; `merge` łączy rozszerzenia (jeden pipeline dla całej partii), ale nie łączy rodzajów alfa i modelu dyfuzji (osobne pipeline'y: blending, sortowanie). Bity `MaterialData::flags` powtarzają wariant w danych, dla shaderów, które nie mogą mieć permutacji (hit shader RT rozgałęzia się na nich w czasie wykonania).

### Układ danych i tablica materiałów

- `MaterialData` (144 B, wyrównanie 16, ten sam układ w uniform i storage): pięć `vec4` (kolor podstawowy, emisja × siła, kolor i współczynnik specular, kolor i dystans tłumienia, kolor i współczynnik transmisji dyfuzyjnej), potem 16 skalarów `f32`/`u32` (w tym `flags` i pięć slotów tekstur). Nie ma pól `vec3`, więc nie ma ukrytego dopełnienia; dystans 0 znaczy „nieskończony” (WGSL nie gwarantuje nieskończoności w danych).
- Zgodność CPU ↔ WGSL pilnują testy bez GPU: `reflect_buffer_layout` (nowe w `pathon-shader`) czyta z naga offsety, rozmiary i `stride` struktury z złożonego WGSL, a test porównuje je z `offset_of!` (`MATERIAL_DATA_FIELDS`) i `size_of`; `reflect_bind_groups` potwierdza typy bindingów.
- **Tablica materiałów** to `MaterialTable` nad `GpuArray<MaterialData>` z `pathon-resources` (storage buffer na każdym profilu). `MaterialId(u32)` jest **indeksem logicznym**: ten sam numer wybiera element tablicy w shaderze, slot `binding_array` (profil Desktop) albo bind group (Web) w M4-04. Usunięty indeks wraca do użycia po `frames_in_flight` klatkach (`begin_frame`, ADR-0022), bo lista nagrana, a niezgłoszona, może go jeszcze czytać.
- **Tekstury to indeksy logiczne** (`TextureSlot(u32)`, `NO_TEXTURE` = `u32::MAX`), a jedynym miejscem odczytu jest `sample_material_texture`; do M4-04 domyślny moduł zwraca biel, więc materiały z teksturami dają kolory z samych współczynników.

### Diagnostyki i test generyczności

Kody mają prefiks `Y` (`T` należy do `pathon-profile`): `Y0001` parametr poza zakresem (`StandardMaterial::validate`, zakresy z glTF), `Y0002` nieznany albo usunięty materiał, `Y0003` hook, który się nie kompiluje.

Dowodem, że „ta sama funkcja” jest prawdą, jest test GPU `tests/evaluation.rs`: jeden shader z `shade(punkt)` wołany z compute i z fragmentu (piksel na wynik, cel `Rgba32Float`) daje te same wartości BSDF we wszystkich 48 permutacjach (tolerancja względna 1e-4), a permutacja bez rozszerzeń zgadza się z implementacją równań glTF w Rust (2e-3).

## Konsekwencje

- Rdzeń nie zmienia się; `pathon-shader` dostaje jedną funkcję (`reflect_buffer_layout`), więc M5-07 (`cargo-semver-checks`) obejmuje ją razem z resztą API.
- Zmiana układu `MaterialData` łamie shadery paczek (są po stronie WGSL), więc rozszerzenia dopisujemy tylko na końcu struktury i zmieniamy rozmiar w `size_of` oraz w teście układu.
- 48 permutacji to 48 modułów w keszu (ADR-0023); kompilacja w tle (ADR-0024) zbuduje tylko te, których sceny używają. `MaterialVariant::all()` służy do rozgrzewki i testów.
- Brak hooka w kolejności: dwie paczki z `modify_surface` nie złożą się same. Gdy pojawi się taka potrzeba, hook zamieni się w listę funkcji składaną przez `pathon-material`, bez zmiany sygnatur.
- Złożony WGSL nie ma komentarzy, więc kontrakty (jednostki, konwencje wektorów) opisuje [docs/crates/pathon-material.md](../crates/pathon-material.md).
- Konstanta ambient w przykładzie `material_sphere` jest zastępnikiem IBL; metale bez IBL są ciemne poza odbiciem światła (M4-06).
