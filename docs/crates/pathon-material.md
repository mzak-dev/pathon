# pathon-material

Materiał standardowy (M4-03, [ADR-0030](../adr/0030-material-standardowy-i-hooki-wgsl.md)): model glTF metallic-roughness z rozszerzeniami KHR jako **funkcje WGSL oddzielone od entry pointu**, ta sama funkcja dla fragmentu forward, compute po visibility bufferze i punktu trafienia RT. Do tego opis materiału po stronie CPU, układ danych na GPU, tablica materiałów z indeksem logicznym i flagi permutacji shadera.

To zwykła paczka: zależy tylko od `pathon-api` i `pathon-shader` (`[package.metadata.pathon] kind = "pack"`). Nie rysuje niczego sama: nie ma pętli po światłach, cieni ani IBL (to `pathon-pbr`, `pathon-shadow`, `pathon-ibl`), nie zna sceny (`pathon-scene`) ani bindless (M4-04).

```rust
# use pathon_material::{MaterialTable, StandardMaterial};
# fn main() -> Result<(), pathon_material::MaterialError> {
let mut table = MaterialTable::new("materials", 2);
let paint = table.add(&StandardMaterial {
    base_color: [0.7, 0.05, 0.05, 1.0],
    metallic: 0.0,
    roughness: 0.4,
    clearcoat: 1.0,
    ..StandardMaterial::default()
})?;
assert_eq!(paint.index(), 0); // element tablicy w shaderze, w M4-04 także slot bindless

let variant = table.variant(paint).expect("live");
assert!(variant.clearcoat && !variant.transmission);
let key = variant.shader_key("forward"); // root `forward` importuje package::material::standard
assert_eq!(key.features.iter().collect::<Vec<_>>(), ["clearcoat"]);
# Ok(())
# }
```

## Użycie w paczce

1. Zarejestruj moduły w `ShaderLibrary` albo `BackgroundCompiler` (`add_module` zastępuje moduł o tej samej ścieżce):
   ```rust,ignore
   for (path, source) in MaterialHooks::default().modules() { compiler.add_module(path, source)?; }
   compiler.add_root("forward", FORWARD_WGSL);
   ```
2. W shaderze głównym zaimportuj funkcje i zadeklaruj binding **sam** (moduły nie mają bindingów):
   ```wgsl
   import package::material::standard::{MaterialData, evaluate, load_surface};
   import package::material::surface::{LightSample, SurfaceInput};
   @group(1) @binding(0) var<storage, read> materials: array<MaterialData>;
   ```
3. Wypełnij `SurfaceInput`, wołaj `load_surface(materials[i], input)` i `evaluate(surface, view, light)`.
4. Każdy materiał ma wariant (`MaterialTable::variant`); klucz shadera to `variant.shader_key("forward")`. Partię rysuj potokiem wariantu, który ją pokrywa (`covers`), np. po `merge`.
5. `MaterialTable::sync(device, release, frame)` wysyła zmieniony zakres i zwraca bufor do bind group.

## Kontrakt WGSL

Moduły: `package::material::surface` (struktury), `standard` (model), `hooks` i `texture_source` (miejsca podmiany). Układ wejść i konwencje:

| Typ | Pola i konwencje |
| --- | --- |
| `SurfaceInput` | `position` (świat), `normal` (interpolowana, jednostkowa), `tangent` (`xyz` jednostkowy, `w` ±1 = znak bitangenta), `uv`, **`uv_dx`, `uv_dy`** (jawne gradienty UV w pikselach), `color` (kolor wierzchołka, `1` gdy brak), `front_facing` |
| `Surface` | rozwiązane parametry: `geometric_normal` i `normal` (po odwróceniu dla tylnej strony i mapie normalnych), `base_color`, `alpha`, `alpha_cutoff`, `metallic`, `roughness` (percepcyjna, ≥ 0,045), `occlusion`, `emissive`, `diffuse_color`, `f0`, `f90`, `clearcoat*`, `transmission`, `thickness`, `attenuation_*`, `diffuse_transmission*` |
| `LightSample` | `direction` (jednostkowy, **od powierzchni do światła**), `radiance` (radiancja padająca, już po tłumieniu odległością i cieniu) |
| `BsdfResult` | `diffuse`, `specular`: radiancja wychodząca w kierunku `view` od jednej próbki (BRDF · `radiance` · cos θ), osobno |

| Funkcja | Opis |
| --- | --- |
| `load_surface(material, input) -> Surface` | czyta współczynniki i tekstury, liczy `f0`, `f90`, `diffuse_color`, normalną; na końcu woła `modify_surface` |
| `evaluate(surface, view, light) -> BsdfResult` | `view` to jednostkowy wektor od powierzchni do oka; tylna strona z `subsurface` dostaje transmisję dyfuzyjną w `diffuse` |
| `transmission_weight(surface, view) -> vec3` | waga światła załamanego; wołający mnoży ją przez to, co leży za powierzchnią |
| `volume_transmittance(surface, distance) -> vec3` | Beer–Lambert; dystans tłumienia 0 daje 1 |
| `passes_alpha_test(surface) -> bool` | prawda poza permutacją `masked`; `discard` należy do entry pointu (compute go nie ma) |
| `output_alpha(surface) -> f32` | `alpha` w permutacji `transparent`, inaczej 1 |

**Bez pochodnych ekranowych.** Materiał nie woła `dpdx`, `dpdy` ani `textureSample`; gradienty przychodzą w `SurfaceInput`. Fragment bierze je z `dpdx(uv)`, compute po visibility bufferze z trójkąta, hit shader z kamery; compute w testach podaje stałe. Dzięki temu funkcje są poprawne w compute i w hit shaderze, gdzie pochodnych nie ma.

**Model.** Lambert z `(1 − F)` (dodatek B glTF) albo Burley (`burley`), GGX (D), Smith height-correlated (V), Schlick (F); `f0 = lerp(min(((ior−1)/(ior+1))² · specular_color, 1) · specular_factor, base_color, metallic)`, `f90 = lerp(specular_factor, 1, metallic)`. Clearcoat to druga warstwa GGX (`F0 = 0,04`, normalna geometryczna) tłumiąca warstwę bazową przez `1 − clearcoat · F`. Transmisja zmniejsza `diffuse` o `1 − transmission`; `subsurface` dzieli diffuse na część przednią `(1 − t)` i tylną `t` z kolorem transmisji.

### Hooki paczek

| Moduł | Sygnatura | Domyślnie |
| --- | --- | --- |
| `package::material::hooks` | `fn modify_surface(input: SurfaceInput, surface: Surface) -> Surface` | tożsamość |
| `package::material::texture_source` | `fn sample_material_texture(slot: u32, uv: vec2<f32>, uv_dx: vec2<f32>, uv_dy: vec2<f32>) -> vec4<f32>` | `vec4(1.0)` (biel) |

Paczka podmienia moduł pod tą samą ścieżką: `MaterialHooks` (typ w `Shared`: `get_or_insert_with(MaterialHooks::default)`) przechowuje źródła, `set_surface_hook` i `set_texture_source` je ustawiają, `modules()` oddaje komplet do `add_module`. Hook może importować resztę `package::material::*`. `validate()` składa moduły bez flag i ze wszystkimi naraz i sprawdza je w naga; błąd to `Y0003`. Hook nie może zależeć od flagi, której nie ma w każdej permutacji. Tylko jeden hook danego rodzaju naraz.

`texture_source` to jedyne miejsce, w którym materiał czyta teksturę; M4-04 podstawi tu `binding_array` (Desktop) albo bind group per materiał (Web) z tym samym `slot`.

## Flagi permutacji

`MaterialVariant::of(&material).shader_key(root)` daje `ShaderKey` z flagami WESL ([ADR-0023](../adr/0023-permutacje-shaderow.md)); nazwy są w module `flags`.

| Flaga | Kiedy | Co zmienia |
| --- | --- | --- |
| *(żadna)* | `AlphaMode::Opaque` | `output_alpha` = 1, `passes_alpha_test` = true |
| `masked` | `AlphaMode::Mask { cutoff }` | `passes_alpha_test`: `alpha >= alpha_cutoff` |
| `transparent` | `AlphaMode::Blend` | `output_alpha` = `alpha` |
| `subsurface` | `diffuse_transmission > 0` | lobe transmisji dyfuzyjnej (`KHR_materials_diffuse_transmission`) |
| `transmission` | `transmission > 0` | `1 − transmission` na `diffuse`; `thickness`, tłumienie objętości (`KHR_materials_transmission` i `volume`) |
| `clearcoat` | `clearcoat > 0` | warstwa clearcoat |
| `burley` | `DiffuseModel::Burley` | dyfuzja Burleya zamiast Lamberta |

`masked` i `transparent` się wykluczają: 3 rodzaje alfa · 2⁴ = **48** wariantów (`MaterialVariant::all()`, `MaterialVariant::COUNT`). `merge` łączy rozszerzenia dwóch wariantów tego samego rodzaju alfa i modelu dyfuzji (jeden potok na partię), `covers(needed)` sprawdza, czy potok wariantu rysuje dany materiał poprawnie. Flaga wyłączona usuwa kod z modułu (współczynniki rozszerzeń zostają w `MaterialData`, ale są ignorowane), więc potok bez `transmission` rysuje materiał z transmisją jako nieprzezroczysty: wariant zawsze bierz z materiału. Bity `MaterialData::flags` (`flag_bits`) powtarzają te same informacje w danych, dla shaderów bez permutacji.

## Model CPU i dane GPU

`StandardMaterial` (pola publiczne, `Default` = wartości domyślne glTF, w tym `metallic = 1`, `roughness = 1`): `base_color`, `metallic`, `roughness`, `normal_scale`, `occlusion_strength`, `emissive` × `emissive_strength`, `alpha_mode`, `double_sided`, `diffuse_model`, `ior`, `specular_factor` / `specular_color`, `clearcoat` / `clearcoat_roughness`, `transmission`, `thickness`, `attenuation_color` / `attenuation_distance` (`None` = nieskończony), `diffuse_transmission` / `diffuse_transmission_color`, `textures` (`TextureSlot(u32)` na pięć map). `validate()` sprawdza zakresy glTF (`Y0001`).

`MaterialData` (`#[repr(C)]`, 144 B, wyrównanie 16; w WGSL `MaterialData` w `standard`):

| Offset | Pole |
| --- | --- |
| 0 | `base_color: vec4` |
| 16 | `emissive: vec4` (rgb × siła, a = 0) |
| 32 | `specular_color: vec4` (a = `specular_factor`) |
| 48 | `attenuation_color: vec4` (a = dystans, 0 = nieskończony) |
| 64 | `diffuse_transmission_color: vec4` (a = współczynnik) |
| 80 | `metallic`, `roughness`, `ior`, `normal_scale`, `occlusion_strength`, `clearcoat`, `clearcoat_roughness`, `transmission`, `thickness`, `alpha_cutoff` (po 4 B, do 116) |
| 120 | `flags: u32` |
| 124 | `base_color_texture`, `metallic_roughness_texture`, `normal_texture`, `occlusion_texture`, `emissive_texture` (`u32`, `NO_TEXTURE` = `u32::MAX`; do 144) |

Rozszerzenia dopisuj na końcu struktury (po obu stronach naraz). `MATERIAL_DATA_FIELDS` to nazwy i offsety Rust w kolejności WGSL.

### Tablica materiałów

`MaterialTable` to `GpuArray<MaterialData>` (storage buffer na każdym profilu) z indeksem logicznym `MaterialId(u32)`:

- `add(&material)`, `set(id, &material)`, `remove(id, frame)`, `get`, `variant`, `len`, `live_count`, `buffer`;
- usunięty indeks wraca do użycia po `frames_in_flight` klatkach (`begin_frame(frame)`, [ADR-0022](../adr/0022-zegar-klatki-paczek.md)); użycie starego id to `Y0002`;
- `sync(device, release, frame)` wysyła tylko zmieniony zakres; `forget_gpu_buffer()` po utracie urządzenia.

Ten sam numer jest elementem tablicy w shaderze i indeksem materiału dla bindless (M4-04): tam ten indeks wybiera slot `binding_array` albo bind group.

## Diagnostyka

| Kod | Sytuacja |
| --- | --- |
| [Y0001](../diag/Y0001.md) | parametr materiału poza zakresem (NaN, współczynnik spoza 0..=1, `ior` < 1…) |
| [Y0002](../diag/Y0002.md) | nieznany albo usunięty materiał |
| [Y0003](../diag/Y0003.md) | hook nie kompiluje się (w żadnej z permutacji) |

`MaterialError::to_diagnostic()` daje `Diagnostic` z podpowiedzią.

## Testy i przykład

- `tests/layout.rs` (bez GPU): `reflect_buffer_layout` (z `pathon-shader`) czyta z naga offsety, rozmiary i stride `MaterialData` ze złożonego WGSL i porównuje z `offset_of!` / `size_of` w Rust; `reflect_bind_groups` potwierdza typy i widoczność bindingów compute i fragmentu.
- `tests/permutations.rs` (`rhi-null`, bez GPU): wszystkie 48 permutacji składają się, przechodzą walidację naga i tworzą moduł na urządzeniu null, każda daje inny tekst; flagi usuwają kod, który osłaniają; `BackgroundCompiler` buduje wszystkie 48; hook podmienia moduł.
- `tests/evaluation.rs` (`--features gpu-tests`): jeden shader z funkcją `shade(punkt)` jest wołany z **compute** (wątek na punkt) i z **fragmentu** (piksel na wynik, cel `Rgba32Float`); BSDF z obu ma zgadzać się w tolerancji względnej 1e-4 we wszystkich 48 permutacjach. Dodatkowo: permutacja bez rozszerzeń zgadza się z implementacją równań glTF w Rust (2e-3), każda flaga włącza dokładnie swój lobe, `masked` / `transparent` dają oczekiwane `alpha` i test, a oba hooki (powierzchni i tekstur, z analitycznie sprawdzonym slotem i gradientem) działają w obu etapach. Backend wybiera `PATHON_TEST_BACKEND=vulkan|dx12|metal`.

```sh
cargo test -p pathon-material                                     # bez GPU
cargo test -p pathon-material --features gpu-tests -- --test-threads=1   # PowerShell: $env:PATHON_TEST_BACKEND="vulkan"
cargo run -p pathon-examples --bin material_sphere [plik.png]
```

Przykład `material_sphere` ([przykłady](../examples.md)): siatka 5 × 3 kul (dielektryk, metal, lakier z clearcoatem; chropowatość rośnie w kolumnach) shadowana przez `load_surface` + `evaluate`, jedno światło kierunkowe i stały ambient w miejsce IBL; wynik trafia do PNG.
