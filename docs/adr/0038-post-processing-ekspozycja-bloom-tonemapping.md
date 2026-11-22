# ADR-0038: Post-processing — ekspozycja, bloom i tonemapping SDR

**Status:** Przyjęta
**Data:** 2026-11-22
**Dotyczy:** M4-09, [ADR-0011](0011-bufory-i-tekstury-trwale-w-grafie.md) (historia w grafie), [ADR-0012](0012-kontrakt-paczki-w-pathon-api.md) (kontrakt paczki), [ADR-0018](0018-immediates-i-overrides.md) (overrides, brak immediates na webie), [ADR-0022](0022-zegar-klatki-paczek.md) (czas od hosta), [ADR-0025](0025-golden-images.md) (golden images); zostawia punkt rozszerzenia dla M4-16 (wyjście HDR)

## Kontekst

Pierwsza pełna klatka (★) kończy się obrazem w `output`: ktoś musi zamienić liniowe `hdr` w jasność ekranu. Kontrakt slotów (`docs/slots.md`) mówił „`hdr` jest liniowe i pre-exposed, mnożnik w slocie `exposure`”, co się wzajemnie wyklucza: jeśli `hdr` jest już przemnożone, mnożnik nie ma po co istnieć. Trzeba rozstrzygnąć znaczenie, wybrać model ekspozycji, kształt bloomu i dwa operatory tonemappingu, a przy tym nie blokować wyjścia HDR (M4-16, równolegle `SurfaceColorSpace` w RHI) i nie zmieniać `pathon-api`.

## Decyzja

### Jedna paczka, trzy `RenderFeature`

`PostPack` rejestruje `ExposureFeature`, `BloomFeature` i `TonemapFeature` (`Pack::register`); każdą można też zarejestrować osobno. Zależy tylko od `pathon-api` (`pack-boundary`). `pathon-api` nie zmienia się: istnieją `Role::TONEMAP` i `Stage::Tonemap` (F0102 pilnuje jednego posiadacza roli). Ustawienia (`PostSettings`) leżą w `Shared`, więc aplikacja zmienia je między klatkami; wartości poza zakresem są naprawiane i zgłaszane raz jako Q0001.

### Znaczenie `hdr` i `exposure`

`hdr` to liniowa luminancja sceny w jednostkach producenta (fizycznych, gdy scena je ma), **bez** mnożnika ekspozycji. `exposure` (`R32Float` 1×1) to mnożnik, po którym `hdr × exposure` jest jasnością wyświetlaną (0.18 = środkowa szarość). Pre-exposure producenta (mnożenie wyniku oświetlenia przez ekspozycję poprzedniej klatki, żeby zmieścić zakres w fp16) jest otwartą optymalizacją dla pbr (M4-05 i dalej); gdy się pojawi, producent opisze ją w `Shared`, a `exposure` zostanie mnożnikiem *pozostałym*. Do tego czasu `docs/slots.md` mówi to, co robi kod. Slot `exposure` zapisuje `ExposureFeature` w `Stage::PostProcess`, a czytają go bloom (próg działa na jasności po ekspozycji) i tonemapping.

### Ekspozycja

- **Ręczna:** `ExposureMode::Manual { ev }`, mnożnik `2^-ev`, plus `compensation_ev` (dodatnie rozjaśnia) w obu trybach. Nie przyjmujemy skali EV100 aparatu: jej `1.2 × 2^EV100` łączy kalibrację czułości z konwencją nasycenia, a scena pathon nie musi mieć jednostek fizycznych.
- **Automatyczna:** histogram luminancji w compute. 256 pojemników po `log2` luminancji w `[min_ev, max_ev]` (domyślnie `-9…13`), pojemnik 0 na to, co ciemniejsze (czerń nie ciągnie średniej). Szerokość pojemnika `22/255 ≈ 0.086 EV`, więc błąd średniej dla jednolitej szarej karty to najwyżej pół pojemnika, `0.043 EV` (wymaganie: ±0.1 EV). Zliczanie w pamięci workgroupa i dopiero potem do globalnej tablicy, bo jednolity kadr zamieniłby milion atomików na jeden adres. Średnia po odcięciu percentyli (`0.10…0.95`): kilka bardzo jasnych pikseli nie decyduje o ekspozycji. Docelowa luminancja `target_luminance` (0.18).
- **Adaptacja** w przestrzeni `log2(exposure)` (symetryczna w stopniach), wykładniczo, ze stałymi czasu osobno dla jaśniejącej (`0.3 s`) i ciemniejącej (`1 s`) sceny; współczynnik `1 − exp(−Δt/τ)` liczony na CPU z `ViewCtx::delta_seconds`, rdzeń nie czyta zegara. `Δt = 0` nie rusza ekspozycji, `τ = 0` skacze od razu. Stan to `history_texture` (ADR-0011), więc działa per cel i unieważnia się sam przy zmianie rozmiaru i resecie; w pierwszej klatce (`previous_valid == false`) ekspozycja skacze do celu. `Slot::EXPOSURE` jest wprost `history.current`, bez kopiowania.
- Pamięć na histogram to bufor przejściowy grafu, zerowany jako pierwszy dispatch tego samego passa compute (kolejność dispatchy w jednym passie gwarantuje WebGPU), więc paczka nie trzyma własnych buforów między klatkami.

### Bloom: passy renderujące, nie compute

Piramida downsample/upsample (Jimenez, Call of Duty: AW), każdy poziom to osobna tekstura grafu i pełnoekranowy trójkąt. Powody wyboru passów renderujących: próbkowanie dwuliniowe jest wprost sprzętowe (compute musiałby je robić ręcznie albo przez `textureSampleLevel` w compute, a nie wszystkie stosy mają równy dostęp), `Rgba16Float` jako cel renderowania jest filtrowalny i renderowalny na każdym profilu WebGPU (zapis do storage nie wymaga rozszerzeń, ale zmusiłby do `STORAGE` na każdym poziomie), a każdy poziom jako osobna tekstura omija śledzenie stanów podzasobów mipów (G0008 / R0102) i pozwala puli grafu aliasować pamięć.

- **Poziom 0 = połowa rozmiaru renderu.** Prefilter czyta 6×6 tekseli źródła `textureLoad`-em (nie próbnikiem), bo jeden NaN w `hdr` zatruwa dwuliniowych sąsiadów; każdy teksel jest sanityzowany (NaN i ujemne → 0, `+Inf` → 65504), mnożony przez `exposure`, przycinany do 60000 i filtrowany progiem z miękkim kolanem (`threshold`, `knee`).
- **Karis average** na pierwszym downsample (`karis_average`, domyślnie włączone): pięć grup 13-tapowego filtra ważonych `1 / (1 + luma)`, żeby pojedynczy bardzo jasny piksel nie migotał. To świadomie nieliniowe; test energii wyłącza Karisa.
- **Upsample:** filtr namiotowy węższego poziomu mieszany z ostrzejszym (`mix(ostry, namiot, scatter)`). Każdy krok to kombinacja wypukła, więc piramida zachowuje energię wejścia (test: średnia bloomu = średnia wejścia ±5%). `scatter` (0.7) wybiera, ile z każdego szerszego poziomu dochodzi do wyniku.
- Wynik to `post.bloom` (`Rgba16Float`, połowa rozmiaru renderu, w jednostkach po ekspozycji), który tonemapping dodaje: `kolor = hdr·exposure + bloom·intensity`. Dodawanie (nie `mix`) zostawia `intensity = 0` jako dokładne wyłączenie.

### Tonemapping SDR: ACES fitted i Khronos PBR Neutral

Wybrane dwa, parametrem `Tonemapper`:

- **`AcesFitted`** (Hill; macierze wejścia i wyjścia RRT+ODT i dopasowanie wymiernej krzywej, z przedmnożeniem `1/0.6` jak w three.js, żeby środkowa szarość trafiała blisko połowy jasności ekranu). Filmowy wygląd, który użytkownik silnika zwykle oczekuje od domyślnej krzywej; znane przesunięcia odcieni to jego cecha.
- **`PbrNeutral`** (Khronos). Zachowuje kolory bazowe materiałów poniżej ramienia i kompresuje tylko światła; to wybór dla wizualizacji produktów i scen glTF, gdzie kolor materiału ma wyglądać jak w specyfikacji. Kilka linii kodu, więc ma dokładną referencję CPU.
- **AgX odrzucony na teraz.** Dobry wygląd, ale to trzy macierze, wielomian szóstego stopnia i parametry looków; przy dwóch operatorach z przeciwnych biegunów (filmowy, wierny) nie wnosi nic, czego brakuje, a dokłada kod do utrzymania. Wraca jako trzeci wariant `Tonemapper` (enum jest `#[non_exhaustive]`), jeśli golden image pokaże, że ACES przesadza z odcieniami. Reinhard i Narkowicz odrzucone: pierwszy nie ma ramienia, drugi jest uproszczeniem ACES bez zalet.

### Wyjście: format celu i transfer

`ENCODE_SRGB` to override wybierany z formatu `output` (`texture_format`): format `*Srgb` koduje sprzęt (tak jest też na webie, gdzie powierzchnia jest oglądana przez widok sRGB, poprawka `f20c973`), inne formaty `Unorm` (`Rgba8Unorm`, `Bgra8Unorm`, `Rgb10a2Unorm`) dostają OETF w shaderze, formaty zmiennoprzecinkowe zostają liniowe. Jedna ścieżka wpisana w klucz potoku (format, operator, bloom), bez drugiej wersji shadera.

**Punkt rozszerzenia dla M4-16:** `TonemapSettings::output: OutputTransfer` (`#[non_exhaustive]`) z wariantami `Srgb` (zaimplementowany), `Pq { peak_nits }` i `ScRgb { paper_white_nits }`. Dwa ostatnie zgłaszają Q0002 raz i renderują jak `Srgb`. M4-16 dołoży gałąź w `tonemap.wgsl` (stała `OUTPUT_TRANSFER` obok `OPERATOR`) i wybór wariantu z `SurfaceColorSpace`, bez zmiany pozostałych passów i API ustawień.

### Sanityzacja wejścia

Każdy odczyt `hdr` (histogram, prefilter, tonemapping) czyści piksel: NaN i wartości ujemne → 0, `+Inf` → 65504 (największy half), reszta przycięta. Test bitów zamiast `isnan`, bo WGSL go nie ma, a kompilatory mogą zoptymalizować porównania z NaN. Tonemapping miesza teksele ręcznie po sanityzacji (a nie próbnikiem), więc zepsuty piksel zostaje jednym czarnym pikselem. Wyniki są skończone dla wejścia 0, `Inf` i NaN (testy GPU).

### Parametry bez immediates

Parametry pasów idą przez uniform ring (`UniformRing`, restart na `record_epoch`, ADR-0022), bo przeglądarki nie mają immediates (ADR-0018). Warianty shadera (operator, kodowanie, bloom, Karis) to overrides.

### Diagnostyki `Q`

`Q0001` ustawienie naprawione (przycięte), `Q0002` transfer wyjściowy niezaimplementowany, `Q0003` brak slotu `exposure` (tonemapping pominięty).

## Konsekwencje

- `docs/slots.md` zmienia zdanie o `hdr`: wartość wyświetlana to `hdr × exposure`; pre-exposure producenta jest odroczone.
- Tonemapping nadpisuje cały `output` (czyści go): host w trybie A z obrazem do zachowania powinien kopiować wynik, nie podkładać `output` pod scenę. Overlaye (debug, UI) idą po etapie `Tonemap` i dobierają `output` przez `modify`.
- Paczka wymaga `R32Float` zapisywalnego z compute i filtrowalnego `Rgba16Float` jako celu renderowania (baseline WebGPU); inaczej negocjacja wyłącza ją z F0101.
- Golden images (`post_chain_aces`, `post_chain_neutral`) pochodzą z jednego urządzenia deweloperskiego; zgodność lavapipe, WARP i Metala w progu `cross_backend()` potwierdzi dopiero CI (ADR-0025).
- Histogram liczy po `render_size`, przed ewentualnym upscalingiem (TemporalResolve, M5): ekspozycja widzi obraz sprzed rekonstrukcji, co jest tym, co chcemy.
- Nie ma metrażu środka kadru ani maski wag: średnia jest globalna. Dojdzie jako ustawienie, gdy pojawi się potrzeba.
- Otwarte: pre-exposure producenta, AgX, metraż ważony, dithering przed kwantyzacją 8-bitową (pasmowanie ciemnych gradientów), bloom na rozmiarze po upscalingu.
