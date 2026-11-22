# ADR-0031: Tekstury — wybór kodowania, sRGB w compute i KTX2

**Status:** Przyjęta
**Data:** 2026-11-22
**Dotyczy:** M4-13, [ADR-0020](0020-kontrakty-rhi-types.md) (zapis tekstur z hosta), [ADR-0023](0023-permutacje-shaderow.md) (granice paczek)

## Kontekst

Ta sama scena ma wyglądać tak samo na GPU, które próbkuje BCn (desktop), ASTC / ETC2 (mobile, Apple) i na takim, które nie próbkuje żadnego skompresowanego formatu (część przeglądarek). Do tego potrzebne są mipmapy dla tekstur, które przychodzą bez nich, a poprawność kolorystyczna nie może zależeć od tego, czy tekstura jest sRGB, liniowa czy mapą normalnych. Paczki w `pathon-material` (M4-03) i glTF (M4-10) dostają teksturę z jednego miejsca, więc jej kontrakt trzeba ustalić przed nimi.

## Decyzja

### Źródło niesie kilka kodowań, wybór jest czystą funkcją `Caps`

- `TextureSource` to jeden obraz w **wariantach** (ten sam obraz w BC7, ASTC, RGBA8…). `TextureLoader::choose_format(&Caps, &TextureSource)` wybiera wariant bez urządzenia i bez danych, więc ma testy tabelaryzowane na zmyślonych `Caps` (desktop, web, iOS-podobne).
- Wariant jest użyteczny, gdy macierz `Caps::formats` daje mu `SAMPLED | FILTERABLE` i (dla skompresowanych) włączony jest bit `TEXTURE_COMPRESSION_*`: bity są źródłem prawdy, nie nazwa platformy. Priorytet kodowań: BC, ASTC, ETC2, nieskompresowane; remis rozstrzyga kolejność wariantów w źródle. Brak użytecznego wariantu to błąd `X0001`, nie ciche przekodowanie.
- **Bez dekodera CPU dla BCn.** Źródło, które ma być wczytane wszędzie, niesie też wariant nieskompresowany. Dekoder BC7 (wszystkie tryby) i enkoder to osobny, duży kawałek pracy bez użytkownika w M4; zależy od niego tylko wygoda źródeł z jednym kodowaniem.
- Brak formatów ETC2 sRGB w `Format` (M1-01) znaczy, że źródło sRGB nie użyje wariantu ETC2. Dodanie ich do `pathon-rhi-types` jest zmianą w RHI i backendach, poza zakresem M4-13; ETC2 działa dla danych liniowych i normal map.

### sRGB: przechowywane liniowo, próbkowane przez widok

Tekstury `Rgba8UnormSrgb` nie mają `STORAGE`, więc compute nie mógłby pisać ich mipów. 8-bitowy kolor sRGB jest więc alokowany jako **`Rgba8Unorm` z `view_formats = [Rgba8UnormSrgb]`** i próbkowany przez widok sRGB (`Texture::view_desc()`); skompresowane formaty sRGB (BC7, BC1, BC3, ASTC) zostają same sobą. `FormatChoice` rozróżnia `storage` i `sampling`. Shader mipmap czyta wartości zapisane (`Rgba8Unorm`) i **sam** dekoduje sRGB, uśrednia w świetle liniowym i koduje, więc wynik nie zależy od tablicy dekodującej sprzętu, a CPU ma identyczną referencję.

### Mipmapy: compute, jeden dispatch na poziom

- Poziom *N* z poziomu *N − 1*, `8×8` na dispatch, bariery `STORAGE_WRITE → SAMPLED` między poziomami; stan końcowy to `SAMPLED`. Formaty: `Rgba8Unorm`, `Rgba16Float` (te, które WebGPU pozwala zapisać bez rozszerzeń).
- **Filtr box** o obszarze pokrywanym przez teksel docelowy (2×2 przy połowieniu, szerszy przy nieparzystym rozmiarze). Jądro Kaiser odrzucone na teraz: dla jednego przejścia box jest standardem silników, a Kaiser wymaga szerszego, rozdzielnego przejścia i nie zmienia kontraktu; dojdzie jako tryb, jeśli golden image pokaże aliasing.
- **Tryby:** `Linear` (średnia), `Srgb` (patrz wyżej, alfa liniowo), `Normal` (średnia wektorów `n = rgb·2−1`, renormalizacja, zerowy wektor → `(0,0,1)`, zapis `n·0.5+0.5`). Tryb wynika z `ColorSpace` źródła.
- Urządzenie bez `STORAGE` dla formatu liczy łańcuch na CPU tą samą arytmetyką (`reference::generate_chain`) i wgrywa go przez `write_texture`. Ta sama implementacja jest referencją testów GPU (tolerancja ±1 dla RGBA8).
- Skompresowany wariant z jednym poziomem zostaje z jednym poziomem (brak dekodera → brak uśredniania), poziomy skompresowane przychodzą z potoku zasobów.

### Upload i stan

`write_texture` po jednym wywołaniu na poziom (wszystkie warstwy); `bytes_per_row` bez wyrównania, a kopia skompresowanego poziomu mniejszego niż blok obejmuje cały blok (ADR-0020). Po `write_texture` poziomy są w `COPY_DST`, a loader kończy listą z barierą do `SAMPLED`: `Texture::STATE` to `initial` przy imporcie do grafu. Loader sam zgłasza komendy (`DeviceApi` jest wielowątkowe), więc działa w `setup` i na wątku ładującym.

### KTX2 jako moduł z feature, nie osobny crate

`pathon-ktx2` jako osobny crate łamałby regułę `pack-boundary` (paczka zależy z crate'ów pathon tylko od `pathon-api` i `pathon-shader`), a nie ma zewnętrznych zależności, które uzasadniałyby wydzielenie. Parser jest modułem `pathon-texture` za `feature = "ktx2"` (domyślnie włączonym); nie dokłada niczego do domknięcia rdzenia. Zakres: nagłówek, indeks poziomów, `vkFormat` → `Format`, warstwy i ściany (cube), `levelCount = 0` jako „wygeneruj mipy”. **Poza zakresem:** supercompression (BasisLZ i transkoder UASTC, Zstd, ZLIB) — Zstd wymagałby dekodera (`ruzstd`, kilka tysięcy linii) za jeden z kilku wariantów, a BasisLZ transkodera; koszt nie jest mały, więc `X0004` z podpowiedzią o reeksporcie bez supercompression.

### Diagnostyka

Prefiks `X`: `X0001` brak próbkowalnego wariantu, `X0002` niepoprawne źródło, `X0003` tekstura większa niż limit urządzenia, `X0004` kontener KTX2, `X0005` brak generatora mipmap dla formatu, `X0006` ostrzeżenie o fallbacku.

## Konsekwencje

- `pathon-material` i loader glTF (M4-10) dostają teksturę przez `TextureSource`; muszą znać `ColorSpace` tekstury (albedo sRGB, normalne `Normal`, reszta `Linear`) i próbkować przez `Texture::view_desc()`, inaczej sRGB zostanie odczytane liniowo.
- Test goldena BC7 vs fallback RGBA8 pilnuje, że oba warianty dają identyczny obraz na jednym sprzęcie; bit-dokładność BC7 sprzęt–dekoder potwierdzona na Vulkanie (RX 9070 XT). Pierwszy przebieg CI (lavapipe, WARP, Metal bez BCn) potwierdzi referencję `texture_bc7_mips` na pozostałych stosach; na Metalu test wykona tylko połowę fallbackową.
- Otwarte: dekoder / enkoder BCn na CPU, jądro Kaiser, mipmapy tekstur 3D, formaty ETC2 sRGB w `Format`, supercompression KTX2, generowanie mipów dla `Rg8` / `R8` / `Bgra8` (dziś `X0005`: dostarcz łańcuch).
