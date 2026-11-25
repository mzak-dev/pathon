# pathon-ibl

Oświetlenie z otoczenia (M4-06, [ADR-0037](../adr/0037-ibl-srodowisko-prefiltr-i-lut.md)): obraz HDR w pamięci staje się kostką prefiltrowaną per chropowatość, kostką irradiancji i tablicą BRDF (split-sum), a moduły WGSL `package::ibl::*` oddają z nich `diffuse` i `specular` osobno, tak jak `material::evaluate`. **Bez tego metale są czarne**: nie mają światła rozproszonego, a odbijać nie mają czego.

To zwykła paczka (`[package.metadata.pathon] kind = "pack"`): zależy tylko od `pathon-api` (moduły WESL składa konsument przez `pathon-shader`). Nie rysuje niczego sama: nie ma pętli po światłach, cieni, okluzji ani nieba (to `pathon-pbr`, `pathon-shadow`, [`pathon-sky`](pathon-sky.md)), nie czyta plików (wczytanie `.hdr` jest w `examples/`) i nie zna materiałów poza kontraktem `diffuse` / `specular`.

```rust
# use pathon_ibl::{IblBuilder, IblSettings, SyntheticSky};
# fn main() -> Result<(), pathon_ibl::IblError> {
# let device = pathon_rhi::NullDevice::new();
// Dane HDR w pamięci: tu syntetyczne niebo, normalnie `EquirectImage::from_rgb_f32(szerokość, wysokość, &piksele)`.
let image = SyntheticSky::default().image(256, 128)?;

let mut builder = IblBuilder::new();
let environment = builder.build_environment(&device, &image, &IblSettings::COMPACT)?;
assert_eq!(environment.prefiltered_levels, 5); // kostka 128: 128, 64, 32, 16, 8 teksel na ścianę

// Tablica BRDF jest wspólna dla środowisk jednego buildera i liczona raz.
let second = builder.build_environment(&device, &image, &IblSettings::COMPACT)?;
assert_eq!(second.brdf_lut, environment.brdf_lut);

environment.release(&device);
second.release(&device);
builder.release(&device);
# Ok(())
# }
```

## Co powstaje

Wszystko w compute WGSL, w jednym `submit`, jednorazowo (raz przy zmianie obrazu, nie co klatkę); formaty `Rgba16Float`, te same shadery na każdym profilu. Wynik to trwałe zasoby, a nie węzeł grafu (graf nie opisuje cube'a, [ADR-0011](../adr/0011-bufory-i-tekstury-trwale-w-grafie.md)); graf je importuje (`import_texture(.., IblEnvironment::STATE, IblEnvironment::STATE)`).

| Przebieg | Shader | Wynik |
| --- | --- | --- |
| equirect → cube | `equirect_to_cube` | ściana kostki tymczasowej: średnia podpróbek (do 8×8 na teksel), bilinear po stronie shadera, zawinięcie po długości |
| łańcuch mip | `downsample_cube` | filtr box kostki tymczasowej do 1×1 (źródło próbkowania filtrowanego) |
| prefiltr GGX | `prefilter` | `prefiltered`: mip `m` = chropowatość `m / (poziomy − 1)`, mip 0 = ostre środowisko; próbki Hammersleya z rozkładu GGX, waga `n·l`, poziom źródła z kąta bryłowego próbki |
| irradiancja | `irradiance` | `irradiance`: konwolucja kosinusowa, kwadratura po tekselach mipa 32² źródła, normalizowana sumą wag (cubemapa zamiast SH9, powody w ADR-0037) |
| LUT BRDF | `brdf_lut` | `brdf_lut` 2D: R = skala `f0`, G = bias `f90`, osie `n·v` i chropowatość; ta sama widoczność Smitha co `material::evaluate` |

Układ ścian i obrazu: kostka w kolejności +X, −X, +Y, −Y, +Z, −Z (konwencja WebGPU, `t` rośnie w dół); obraz equirect ma biegun +Y w górnym wierszu, a środek obrazu patrzy w −Z (`reference::equirect_direction` / `equirect_uv`). Radiancja jest liniowa; wartości powyżej 65504 są nasycane (binary16), więc bardzo jasne słońce z pliku traci nadmiar energii.

## Wejście: `EquirectImage`

`from_rgb_f32`, `from_rgba_f32`, `from_rgba_f16` (bity binary16) i `from_fn(szerokość, wysokość, |kierunek| radiancja)`. Odrzuca zerowy rozmiar, złą liczbę wartości, NaN i nieskończoność (`I0002`); ujemne wartości zeruje. `SyntheticSky` to niebo z gradientem, ziemią i jasnym słońcem (`furnace(1.0)` daje stałe środowisko do testów energii); to jedyne źródło obrazu, którego używają testy.

## Ustawienia: `IblSettings`

| Pole | Desktop | Compact (Web, mobile) | Znaczenie |
| --- | --- | --- | --- |
| `cube_size` | 256 | 128 | ściana `prefiltered` (i kostki tymczasowej); poziomów `log2 − 2`, ostatni to 8×8 |
| `irradiance_size` | 32 | 16 | ściana kostki irradiancji |
| `prefilter_samples` | 1024 | 256 | próbki GGX na teksel |
| `lut_size`, `lut_samples` | 128, 1024 | 64, 512 | bok i próbki tablicy BRDF |

Rozmiary to potęgi dwójki (`cube_size` 16…2048, pozostałe od 4); inaczej `I0001`. Compact to te same shadery przy czwartej części tekseli, dla profilu Web.

## Kontrakt WGSL

Moduły: `package::ibl::sample` (funkcje próbkujące) i `package::ibl::shading` (`ibl_evaluate`); `pathon_ibl::modules()` oddaje oba do `ShaderLibrary::add_module` / `BackgroundCompiler::add_module`. **Moduły nie deklarują bindingów** (jak [ADR-0030](../adr/0030-material-standardowy-i-hooki-wgsl.md)): shader główny deklaruje zasoby sam i przekazuje je jako argumenty (WGSL pozwala na `texture_cube` i `sampler` w parametrach funkcji).

| Funkcja | Zwraca |
| --- | --- |
| `ibl_diffuse(irradiance, s, n)` | irradiancja / π; `diffuse_color · ibl_diffuse(..)` to radiancja wyjściowa Lamberta (stałe środowisko 1.0 daje 1.0) |
| `ibl_specular(prefiltered, s, r, roughness)` | radiancja odbita po splocie dla `roughness`, `lod = roughness · (poziomy − 1)` |
| `ibl_brdf(lut, s, n_dot_v, roughness)` | `vec2(skala, bias)` |
| `ibl_split_sum(f0, f90, brdf)` | `f0 · brdf.x + f90 · brdf.y`: albedo kierunkowe lobe spekularnego |
| `ibl_dominant_direction(n, r, roughness)` | odbicie przechylone ku normalnej dla chropowatych powierzchni (Lagarde) |
| `ibl_evaluate(irradiance, prefiltered, lut, s, IblInput) -> IblResult` | `IblResult { diffuse, specular }` |

`IblInput { normal, view, diffuse_color, f0, f90, roughness }` to dokładnie pola `Surface` z `material::standard::load_surface` (`view` jest jednostkowy, od powierzchni do oka). `ibl_evaluate` mnoży specular przez `f0 · skala + f90 · bias` i kompensację wielokrotnego rozpraszania (`1 + f0 · (1 / (skala + bias) − 1)`, Fdez-Agüera), a diffuse przez `1 − albedo spekularne`. Wynik to promieniowanie, nie BRDF, więc dodaje się wprost do `BsdfResult` z `evaluate`: `diffuse` do slotu `Slot::DIFFUSE`, `specular` do `Slot::SPECULAR` (po okluzji, którą stosuje `pathon-pbr`).

```wgsl
import package::ibl::shading::{IblInput, ibl_evaluate};
import package::material::standard::{MaterialData, load_surface};
// ... SurfaceInput, binding materiałów ...
IBL_DECLARATIONS   // IblBindings::starting_at(3).wgsl_declarations(0): ibl_irradiance, ibl_prefiltered, ibl_brdf_lut, ibl_sampler

let surface = load_surface(materials[id], input);
let lighting = ibl_evaluate(
    ibl_irradiance, ibl_prefiltered, ibl_brdf_lut, ibl_sampler,
    IblInput(surface.normal, view, surface.diffuse_color, surface.f0, surface.f90, surface.roughness),
);
let radiance = surface.emissive + lighting.diffuse * surface.occlusion + lighting.specular;
```

Po stronie Rust `IblBindings::starting_at(n)` daje cztery kolejne bindingi jednej grupy: `wgsl_declarations(group)` (tekst do shadera głównego), `layout_entries(widoczność)` i `bind_group_entries(&środowisko)`. Test `tests/shaders.rs` pilnuje, że deklaracje i układ są zgodne (przez refleksję naga).

**Nie robi:** okluzji (`surface.occlusion`, AO) i okluzji spekularnej, horizon fade, drugiej próbki dla clearcoata (wołający może dodać `ibl_specular` z `0,04` i `clearcoat_roughness`), wagi transmisji i SSS, światła punktowego ani cieni.

## Środowisko i jego życie

`IblEnvironment` to uchwyt `Copy` (`irradiance`, `prefiltered`, `brdf_lut` jako `IblTexture { texture, view }`, `sampler`, `prefiltered_levels`, `generation`). Mapy są w stanie `IblEnvironment::STATE` (`Access::SAMPLED`). `release(&device)` niszczy to, co środowisko posiada (obie kostki i widoki); tablica BRDF i sampler należą do `IblBuilder` i znikają z `IblBuilder::release`. Gdy klatka w locie może jeszcze czytać mapy, oddaj `owned_resources()` do `RecordCtx::release_later` (ADR-0022).

### `IblPack`

```rust,ignore
let image = Arc::new(EquirectImage::from_rgb_f32(width, height, &pixels)?);
let renderer = Renderer::builder()
    .with_pack(IblPack::new())
    .with_shared(IblSource::new(image, IblSettings::DESKTOP))
    .build()?;
// później: renderer.shared_mut().insert(IblSource::new(inny_obraz, ustawienia));
// paczka lighting czyta renderer.shared().get::<IblEnvironment>()
```

`IblPack` w `record` pierwszej klatki po zmianie `IblSource` (tożsamość `Arc`, ustawienia) buduje środowisko, publikuje `IblEnvironment` w `Shared` i zwalnia poprzednie przez `release_later`. Błąd (`I0001`…`I0004`) trafia do `FrameReport::diagnostics`, nie jest ponawiany co klatkę i zostawia poprzednie środowisko. `negotiate` wyłącza paczkę (`F0101`), gdy `Rgba16Float` nie ma `STORAGE | FILTERABLE`; po utracie urządzenia stan jest czyszczony, a środowisko budowane od nowa.

## Diagnostyka

| Kod | Sytuacja |
| --- | --- |
| [I0001](../diag/I0001.md) | złe ustawienia (rozmiar nie jest potęgą dwójki, zakres, próbki) |
| [I0002](../diag/I0002.md) | zły obraz (rozmiar, długość danych, NaN, nieskończoność) |
| [I0003](../diag/I0003.md) | urządzenie nie zapisuje `Rgba16Float` z compute albo ma za małe limity |
| [I0004](../diag/I0004.md) | obraz większy niż `max_texture_dimension_2d` |

`IblError::to_diagnostic()` daje `Diagnostic` z podpowiedzią.

## Testy i przykład

Referencje CPU są w `pathon_ibl::reference` (publiczne, w `f64`): całki liczone wprost z definicji, a nie ta sama ścieżka co shader (`brdf_lut_texel`, `prefiltered_radiance`, `irradiance_over_pi`, mapowanie ścian i obrazu).

- `src` (bez GPU): konwersja binary16, walidacja obrazu i ustawień, deklaracje bindingów, każdy shader compute przechodzi walidację naga i ma układ zgodny z layoutem (refleksja).
- `tests/shaders.rs` (bez GPU): moduły `ibl::*` składają się obok `material::standard` we wszystkich 48 permutacjach materiału, bindingi z refleksji zgadzają się z `IblBindings`, moduł tworzy się na urządzeniu null.
- `tests/pack.rs` (urządzenie null przez `pathon-core`): `IblPack` buduje środowisko raz na źródło, przebudowuje po zmianie, zgłasza `I0001` raz i zachowuje poprzednie; urządzenie null sprawdza bariery, flagi użycia i bind groupy każdej listy komend.
- `tests/environment.rs` (`--features gpu-tests`): LUT BRDF względem całki CPU (błąd ≤ 0,006, tolerancja 0,01), cache tablicy, prefiltr względem całki CPU w 24 tekselach na mipach 0, 1, 2, 4 i 5 (tolerancja 6% + 0,02, zmierzone do 7,7% na mipie 8×8), irradiancja względem całki CPU (zmierzone 0,7%, tolerancja 3%), **biały furnace** (środowisko 1.0: każdy teksel irradiancji i każdego mipa prefiltru w 1 ± 2%).
- `tests/shading.rs` (`gpu-tests`): `ibl_evaluate` w compute: diffuse albedo 1 daje 1 ± 2%, biały metal 1 ± 3% (chropowatość 0,05…1, `n·v` 0,2…1), dielektryk w 0,98…1,06; mapowanie chropowatości na mipy względem całki CPU.
- `tests/golden_metals.rs` (`gpu-tests`): siatka 5 × 3 kul z `material::standard` (dielektryk, złoto, srebro; chropowatość 0,08…0,88) oświetlona wyłącznie środowiskiem (bez świateł punktowych), renderowana do `Rgba16Float`. Asercje: metale nie są czarne (średnia luminancja złota > 0,15, srebra > 0,25), kontrast wewnątrz kuli maleje monotonicznie z chropowatością, a energia nie ucieka (średnia luminancja srebra różni się od gładkiego o mniej niż 40%). Obraz po mapowaniu tonów na CPU to golden `tests/golden/ibl_metals.png` (`Tolerance::cross_backend()`).

```powershell
cargo test -p pathon-ibl                                                   # bez GPU
$env:PATHON_TEST_BACKEND="vulkan"; cargo test -p pathon-ibl --features gpu-tests -- --test-threads=1 --nocapture
cargo run -p pathon-examples --bin ibl_spheres -- ibl_spheres.png          # niebo syntetyczne
cargo run -p pathon-examples --bin ibl_spheres -- ibl_spheres.png plik.hdr # własne środowisko (Radiance .hdr)
```

Przykład `ibl_spheres` ([przykłady](../examples.md)): te same kule oświetlone środowiskiem z pliku Radiance `.hdr` (czytnik RGBE z kompresją RLE w `examples/src/radiance.rs`) albo niebem syntetycznym; wynik do PNG.

## Ograniczenia

- Jedno środowisko na raz (bez mieszania i bez sond lokalnych); `IblSource` zmienia się w całości.
- Irradiancja to kostka, nie SH9: sensowne wyjątki (bardzo słaby mobile) wymagałyby drugiej ścieżki; kontrakt `ibl_diffuse(n)` by się nie zmienił.
- Mip 8×8 w chropowatości 1 i interpolacja między poziomami dają do kilku procent błędu względem całki (ADR-0037); to cena sześciu poziomów, a nie wady shadera.
- Referencja goldena na lavapipe, WARP i Metalu powstanie dopiero w CI.
