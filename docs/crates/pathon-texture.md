# pathon-texture

Ładowanie tekstur (M4-13, [ADR-0031](../adr/0031-tekstury-kompresja-i-mipmapy.md)): wybór kodowania według matrycy `Caps`, upload przez RHI, mipmapy liczone w compute i czytnik kontenerów KTX2.

To zwykła paczka: zależy tylko od `pathon-api` (`[package.metadata.pathon] kind = "pack"`), nie wchodzi do domknięcia rdzenia i nie zna wgpu. Nie dekoduje obrazów (PNG, JPEG), nie koduje `BCn` i nie zna materiałów: dane przychodzą jako bajty, a rola „jaki to materiał” należy do `pathon-material`.

```rust
# use pathon_texture::{ColorSpace, TextureLoader, TextureSource, Variant};
# fn main() -> Result<(), pathon_texture::TextureError> {
# let device = pathon_rhi::NullDevice::new();
let pixels = vec![255_u8; 64 * 64 * 4];
let source = TextureSource::new("albedo", 64, 64, ColorSpace::Srgb)
    .with_variant(Variant::rgba8(vec![&pixels]));

let texture = TextureLoader::new().load(&device, &source)?;
assert_eq!(texture.desc.mip_level_count, 7);
let view = texture.create_view(&device)?; // widok sRGB, gotowy do bind group
# let _ = view;
# Ok(())
# }
```

## Źródło: ten sam obraz w kilku kodowaniach

`TextureSource` opisuje obraz (rozmiar, warstwy, `ColorSpace`) i listę **wariantów** (`Variant`): ten sam obraz zakodowany na różne sposoby, każdy jako poziomy mip od największego. Typowy zestaw z potoku zasobów to BC7, ASTC 4×4 i nieskompresowany RGBA8 jako ostatnia deska ratunku. Loader wybiera pierwszy sensowny wariant dla urządzenia i wgrywa tylko jego.

- `ColorSpace::Srgb` — kolor zapisany w sRGB; `Linear` — dane i kolor liniowy; `Normal` — normalne tangent-space zapisane jako `n * 0.5 + 0.5`.
- `Variant::new(format, levels)`; `format` można podać w którejkolwiek odmianie pary sRGB / liniowej, bo o odmianie decyduje `ColorSpace`. Poziom zawiera wszystkie warstwy po kolei (jak w KTX2), upakowane w blokach, bez wyrównania wierszy.
- Wariant z **jednym** poziomem i formatem nieskompresowanym dostaje brakujące mipy z generatora (`without_mip_generation()` wyłącza to). Wariant z kompletnym łańcuchem jest używany tak, jak jest. Skompresowany wariant z jednym poziomem zostaje z jednym poziomem: bez dekodera nie da się uśrednić bloków.
- `TextureSource::validate()` (loader woła je sam) sprawdza rozmiary, długości poziomów i wielokrotność bloku w rozmiarze skompresowanych wariantów (`X0002`).

## Wybór formatu: `TextureLoader::choose_format`

Czysta funkcja `choose_format(&Caps, &TextureSource) -> Result<FormatChoice, TextureError>`, bez urządzenia i bez danych, więc testowana tabelarycznie na zmyślonych `Caps` (desktop, web, iOS-podobne).

Wariant jest **użyteczny**, gdy (1) jego format istnieje w `ColorSpace` źródła, (2) macierz `Caps::formats` daje mu `SAMPLED | FILTERABLE` (dla skompresowanych dodatkowo musi być włączony bit `TEXTURE_COMPRESSION_*`), (3) tekstura mieści się w `max_texture_dimension_2d` (inaczej `X0003`). Spośród użytecznych wygrywa kodowanie o najwyższym priorytecie: **BC, ASTC, ETC2, nieskompresowane**; remis rozstrzyga kolejność wariantów w źródle.

| Urządzenie | Źródło: BC7, ASTC, ETC2, RGBA8 | Wynik |
| --- | --- | --- |
| desktop (BC) | sRGB | `Bc7RgbaUnormSrgb` |
| iOS / Apple GPU (ASTC, ETC2, bez BC) | sRGB | `Astc4x4UnormSrgb`, `fallback = true` |
| web bez kompresji | sRGB | `Rgba8Unorm` widziany jako `Rgba8UnormSrgb`, `fallback = true` |

`FormatChoice` niesie `variant` (indeks), `storage` (format alokacji), `sampling` (format widoku, którym próbkuje materiał) i `fallback` (źródło miało bardziej zwarte kodowanie, którego to GPU nie próbkuje; `fallback_diagnostic` daje ostrzeżenie `X0006`). Gdy żaden wariant nie jest użyteczny: `X0001`.

Ograniczenia, które wynikają z listy formatów w `pathon-rhi-types`:

- 8-bitowy kolor sRGB jest przechowywany jako **liniowy `Rgba8Unorm` z `view_formats = [Rgba8UnormSrgb]`**, a próbkowany przez widok sRGB (`Texture::view_desc()`/`create_view()`). Powód: tekstury sRGB nie mają `STORAGE`, więc compute nie mógłby do nich pisać mipów.
- Brak formatów ETC2 sRGB: źródło sRGB nie użyje wariantu ETC2 i spada na kolejny. BC4, BC5 i BC6H (dane liniowe, HDR) działają w `Linear` i `Normal`.
- Nie ma dekodera CPU dla `BCn`: źródło musi nieść wariant, który GPU próbkuje (zwykle RGBA8). Brak takiego wariantu to `X0001`, nie ciche przekodowanie.

## Upload

`TextureLoader::load(&dyn DeviceApi, &TextureSource) -> Texture`:

1. tworzy teksturę (`SAMPLED | COPY_DST`, `STORAGE` tylko gdy mipy idą przez compute, plus `extra_usage` ze źródła, np. `COPY_SRC` do odczytu);
2. wgrywa poziomy wariantu przez `write_texture`, po jednym wywołaniu na poziom ze wszystkimi warstwami. `bytes_per_row` to wiersz bloków bez wyrównania, a rozmiar kopii skompresowanego poziomu mniejszego niż blok jest zaokrąglony w górę do bloku ([ADR-0020](../adr/0020-kontrakty-rhi-types.md));
3. po `write_texture` poziomy są w `COPY_DST`, więc loader zgłasza listę z barierą `COPY_DST → SAMPLED` (a przy generowaniu patrz niżej). Wynik zawsze jest w `Texture::STATE` (`Access::SAMPLED`): to wartość `initial` przy imporcie do grafu klatki.

`load` zgłasza komendy sam, więc nadaje się do `setup` i do wątku ładującego (każda metoda `DeviceApi` jest wielowątkowa). `TextureLoader::release(device)` niszczy pipeline'y mipmap. Tekstura nie jest zwalniana przez loader: należy do wołającego (`device.destroy(texture.id.into())`).

## Mipmapy w compute

`MipmapGenerator` (używany przez loader, dostępny osobno dla własnych tekstur z `SAMPLED | STORAGE`) liczy poziom *N* z poziomu *N − 1* jednym dispatchem `8×8` na warstwę: poziom źródłowy jest `texture_2d`, docelowy `texture_storage_2d<…, write>`, a bariery `STORAGE_WRITE → SAMPLED` rozdzielają poziomy. Formaty: `Rgba8Unorm`, `Rgba16Float` (inne: `X0005`).

- **Filtr: box.** Każdy teksel docelowy uśrednia teksele źródłowe, które pokrywa jego obszar: 2×2 przy połowieniu, a przy nieparzystym rozmiarze szersze pudełka, więc żaden wiersz ani kolumna nie wypada. Jądro Kaiser nie jest zaimplementowane (ADR-0031).
- **`MipMode::Srgb`**: dekodowanie sRGB w shaderze, uśrednianie w świetle liniowym, kodowanie z powrotem (czarno-biała szachownica daje 188, nie 128). Alfa uśredniana liniowo. Dekodowanie robi shader, nie sprzęt, więc wynik nie zależy od tablicy sprzętowej.
- **`MipMode::Normal`**: `n = rgb * 2 - 1`, średnia wektorów, renormalizacja (wektor zerowej długości daje `(0, 0, 1)`), z powrotem do `n * 0.5 + 0.5`. Mipy nie spłaszczają reliefu. Ta sama konwencja dla `Rgba16Float`.
- **`MipMode::Linear`**: zwykła średnia zapisanych wartości.
- Urządzenie bez `STORAGE` dla formatu (albo `TextureLoader::with_cpu_mips()`) liczy łańcuch na CPU tą samą arytmetyką (`pathon_texture::reference::generate_chain`) i wgrywa go przez `write_texture`. To referencja, z którą testy porównują GPU z tolerancją ±1 (RGBA8) i 2·10⁻³ względnie (RGBA16F).

## KTX2 (`feature = "ktx2"`, domyślnie włączona)

`Ktx2Image::parse(&[u8])` czyta kontener [KTX2](https://registry.khronos.org/KTX/specs/2.0/ktxspec.v2.html): format (`vkFormat` → `Format`, z flagą sRGB), rozmiar, warstwy i ściany (sześć = cube), poziomy jako wycinki bajtów pliku. `levelCount = 0` oznacza „wygeneruj resztę”. `image.source("etykieta")` daje `TextureSource` z tym obrazem jako wariantem; fallback dokłada się przez `with_variant`, a dla normal mapy zmienia `color_space` na `Normal` (KTX2 tego nie wie).

```rust,ignore
let image = Ktx2Image::parse(&bytes)?;
let source = TextureSource { color_space: ColorSpace::Normal, ..image.source("rock_normal") }
    .with_variant(Variant::rgba8(vec![&rgba_fallback]));
```

Poza zakresem: **supercompression** (`BasisLZ`, Zstd, ZLIB) → `X0004`, tekstury 3D i wiele innych `vkFormat` (lista w `ktx2.rs`). Parser to moduł tego crate'u, a nie osobny `pathon-ktx2`, bo reguła `pack-boundary` pozwala paczce zależeć od crate'ów pathon tylko od `pathon-api` i `pathon-shader`; nie ma zewnętrznych zależności, więc feature służy tylko do wyłączenia kodu.

## Diagnostyka

| Kod | Sytuacja |
| --- | --- |
| [X0001](../diag/X0001.md) | żaden wariant źródła nie jest próbkowalny na tym urządzeniu |
| [X0002](../diag/X0002.md) | niepoprawne źródło (rozmiar, długości poziomów, wielokrotność bloku) |
| [X0003](../diag/X0003.md) | tekstura większa niż `max_texture_dimension_2d` |
| [X0004](../diag/X0004.md) | niepoprawny albo nieobsługiwany kontener KTX2 |
| [X0005](../diag/X0005.md) | format bez generatora mipmap |
| [X0006](../diag/X0006.md) | ostrzeżenie: użyto fallbacku (mniej zwarty format) |

## Testy

- Jednostkowe: tabela `choose_format` dla `Caps` desktop / web / iOS-podobnych, plan mipmap (compute albo CPU), konwersje f16, parser KTX2 (poprawne pliki i uszkodzone).
- `tests/mipmaps.rs`: łańcuchy sRGB, liniowe, normal map, RGBA16F, nieparzyste rozmiary i wiele warstw, na `NullDevice` (walidacja barier) i z `--features gpu-tests` na prawdziwym urządzeniu, w porównaniu z `reference`, obie ścieżki (compute i CPU).
- `tests/bc7_fallback.rs`: ta sama scena z teksturą BC7 na urządzeniu z `TEXTURE_COMPRESSION_BC` i z fallbackiem RGBA8 na urządzeniu bez niego, oba obrazy porównane z jedną referencją `tests/golden/texture_bc7_mips.png` ([pathon-golden](pathon-golden.md)) i ze sobą. Dane BC7 powstają w teście: mały enkoder trybu 6 i odpowiadający mu dekoder, który wytwarza wariant RGBA8, więc oba warianty niosą identyczne teksele.

```sh
cargo test -p pathon-texture --features gpu-tests
```

Przykład: `cargo run -p pathon-examples --bin texture_load` ([przykłady](../examples.md)).
