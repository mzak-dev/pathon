# ADR-0034: Wyjście HDR — przestrzenie barw powierzchni, `Auto` bez HDR, informacja o wyświetlaczu dla paczek

**Status:** Przyjęta
**Data:** 2026-11-22

## Kontekst

M4-16 wymaga obrazu HDR na monitorze HDR i poprawnego SDR bez niego. Ta decyzja dotyczy części RHI i rdzenia: jak zażądać przestrzeni barw, co powierzchnia może wyświetlić, co się dzieje, gdy nie może, i co dostają paczki. Tonemapping pod HDR (M4-09, `pathon-post`) i weryfikacja na monitorze HDR są poza tą decyzją.

wgpu 30 ma do tego pełne API (źródła: `wgpu-types-30.0.2/src/surface.rs`): `SurfaceConfiguration::color_space`, `SurfaceCapabilities::format_capabilities` (format i zbiór przestrzeni barw, w jakich da się go skonfigurować) oraz `Surface::display_hdr_info` (luminancja w nitach, mnożnik headroomu). wgpu nie robi tone mappingu ani nie ustawia metadanych HDR (`SetHDRMetaData`, `vkSetHdrMetadataEXT`): wartości zapisane do tekstury powierzchni muszą już mieć kodowanie i gamut wybranej przestrzeni.

## Decyzja

### `SurfaceColorSpace` i `SurfaceCaps` w `pathon-rhi-types`

- `SurfaceColorSpace`: `Auto` (domyślna), `Srgb`, `DisplayP3`, `ExtendedSrgbLinear` (scRGB, liniowa, `Rgba16Float`), `ExtendedSrgb`, `ExtendedDisplayP3`, `Hdr10St2084` (BT.2020, PQ, `Rgb10a2Unorm`), `Hlg`. Enum jest `#[non_exhaustive]`; `is_hdr()` jest prawdą dla wszystkich poza `Auto`, `Srgb` i `DisplayP3`. Nazwy i zbiór odpowiadają `wgpu::SurfaceColorSpace`, ale nie jest to ich re-eksport: rdzeń i paczki nie widzą wgpu.
- `SurfaceConfig::color_space` jest **żądaniem**. `DeviceApi::surface_caps(surface)` zwraca `SurfaceCaps`: formaty powierzchni w kolejności preferencji, każdy ze zbiorem `SurfaceColorSpaces` (bitflags), plus `DisplayHdr` (`peak_nits`, `full_frame_nits`, `sdr_white_nits`, `headroom`; `None` znaczy „nieznane”, nigdy zero ani SDR). `DeviceApi::configure_surface` zwraca `SurfaceOutput { format, color_space, display }`: to, co powierzchnia faktycznie wyświetla.
- Wybór jest czystą funkcją na `SurfaceCaps` (`resolve`, `resolve_color_space`, `choose_format`), wspólną dla `rhi-wgpu` i `rhi-null` i testowaną bez GPU.

### Reguły wyboru

1. **`Auto` nigdy nie wybiera HDR** (ani `DisplayP3`): daje `Srgb`, niezależnie od formatu i wyświetlacza. Przejście na HDR zmienia kodowanie, które aplikacja musi napisać do tekstury, więc jest opt-in. Backend wgpu przekazuje zawsze **jawną** przestrzeń: własne `Auto` wgpu wybrałoby scRGB dla `Rgba16Float` i dla żądania `format: Some(Rgba16Float)` bez HDR dawałoby HDR, o który nikt nie prosił.
2. **Jawna przestrzeń, której powierzchnia nie oferuje (dla żadnego formatu), to nie błąd:** powierzchnia dostaje `Srgb`, a rdzeń zgłasza ostrzeżenie **H0001** (nowa seria `H`: wyjście HDR; `docs/diag/H0001.md`). `Auto` i `Srgb` nigdy nie dają H0001.
3. **Format idzie za przestrzenią:** format zażądany przez aplikację, jeśli obsługuje przestrzeń; inaczej format typowy dla przestrzeni (`Rgba16Float` dla rozszerzonych, `Rgb10a2Unorm` dla PQ i HLG); inaczej pierwszy, który ją obsługuje. W SDR wybór zostaje jak dotąd: preferowany format sRGB, a na webie widok sRGB nad formatem Unorm.
4. Konfiguracja jest ponawiana przy zmianie rozmiaru, więc `DisplayHdr` odświeża się z oknem; `Renderer::target_surface_caps` pyta o powierzchnię za każdym razem (okno na innym monitorze, HDR w systemie włączony w trakcie).

### Rdzeń i paczki

- `Renderer::add_window_with_color_space` / `add_surface_with_color_space` / `set_target_color_space` oraz `RendererBuilder::with_color_space` (domyślna przestrzeń okien dodawanych bez nazwy, także przez runner). `Renderer::target_output(id)` zwraca `OutputInfo`, `target_surface_caps(id)` raport powierzchni. Żądana przestrzeń przeżywa zmianę rozmiaru i reset urządzenia.
- `ViewCtx::output: OutputInfo { color_space, display }` — przestrzeń faktycznie wyświetlana (nigdy `Auto`) i wiedza o wyświetlaczu (`peak_nits()`, `headroom`). Cele offscreen i importowane mają `OutputInfo::SDR`. Paczka post (M4-09) wybiera po tym krzywą i kodowanie: `Output` zawiera wartości w kodowaniu `color_space`. **Tonemapping nie jest częścią tego ADR.** Slot `hdr` zostaje liniowy i pre-exposed ([slots](../slots.md)).

### Co wspiera który backend

Wg dokumentacji wgpu 30 oraz raportu `hdr_output` z tej maszyny (Windows 11, RX 9070 XT, monitor SDR 270 nitów, HDR w systemie wyłączony):

| Przestrzeń | DX12 | Vulkan | Metal | Web (WebGPU) |
| --- | --- | --- | --- | --- |
| `Srgb` | tak | tak | tak | tak |
| `DisplayP3` (SDR) | nie | gdy sterownik | tak | tak |
| `ExtendedSrgbLinear` (scRGB, `Rgba16Float`) | tak | gdy sterownik | tak (EDR) | nie |
| `ExtendedSrgb` | nie | gdy sterownik | tak | tak |
| `ExtendedDisplayP3` | nie | nie | tak | tak |
| `Hdr10St2084` (`Rgb10a2Unorm`) | tak | gdy sterownik | tak | nie |
| `Hlg` | nie | gdy sterownik | tak | nie |

`DisplayHdr`: DX12 i Windows podają nity (`peak_nits`, `sdr_white_nits`, `full_frame_nits`); Metal tylko mnożnik headroomu (macOS; główny wątek, poza nim wgpu zwraca puste dane); Vulkan poza Windowsem i web nie podają luminancji (web daje tylko wartość logiczną „HDR możliwy”, a `headroom` jest wtedy `None`). Web: wiersze z dokumentacji wgpu, nie zweryfikowane tutaj.

### Zmierzone na Windows (RX 9070 XT, HDR w systemie wyłączony)

- **DX12:** oferuje `Rgb10a2Unorm: [Srgb, Hdr10St2084]` i `Rgba16Float: [ExtendedSrgbLinear]`, a `DisplayHdr` to 270 nitów szczytu, 80 nitów bieli SDR, `headroom: 1.0`. Czyli **oferta powierzchni nie znaczy, że HDR jest teraz włączony**: o tym mówi `display.headroom` (> 1.0) i `peak_nits`. Aplikacja, która chce HDR tylko na monitorze HDR, sprawdza oba. Żądanie `scRGB` na takim monitorze przechodzi (kompozytor sprowadza obraz do SDR), więc samo „H0001 nie padło” też nie dowodzi HDR.
- **Vulkan (AMD, sterownik 26.11.2):** przy wyłączonym HDR w systemie powierzchnia oferuje tylko `Srgb` (żądanie HDR → H0001 i SDR). Na Windowsie HDR przez Vulkan wymaga więc HDR włączonego w systemie; DX12 jest ścieżką, która pozwala skonfigurować HDR niezależnie od tego.
- `Auto` dało `Srgb` we wszystkich przebiegach, a `DisplayHdr` nie zależy od wyboru.

## Konsekwencje

- `DeviceApi::configure_surface` zmienia typ zwracany (`Format` → `SurfaceOutput`), a `SurfaceConfig` dostaje pole: zmiana łamiąca, dozwolona do zamrożenia w M5-07 ([ADR-0020](0020-kontrakty-rhi-types.md)). Nowa metoda `surface_caps` jest w każdym backendzie, także `rhi-null`.
- `rhi-null` symuje wyświetlacz przez `NullDevice::with_surface_caps(SurfaceCaps)`; domyślnie oferuje sam sRGB i przyjmuje dowolny zażądany format (jak dotąd), więc stare testy się nie zmieniają. Testy rdzenia sprawdzają: `Auto` zostaje SDR na wyświetlaczu HDR, jawna przestrzeń dociera do `ViewCtx::output` z nitami, nieobsługiwana daje H0001 i SDR, żądanie przeżywa resize i reset.
- Przełączenie przestrzeni na żywej powierzchni DX12 (`set_target_color_space` po wyrenderowaniu klatek) zawodziło w wgpu 30.0.2: `ResizeBuffers` zwracał `DXGI_ERROR_INVALID_CALL` („window is in use”), powierzchnia zostawała nieskonfigurowana, a każda klatka kończyła się R0009. Dotyczyło każdej rekonfiguracji (też z identycznymi parametrami i zmiany rozmiaru okna po klatkach), nie tylko HDR. **To był błąd pathona, nie wgpu:** wgpu nie zna widoków powstałych ze `SurfaceTexture` (jego własny `TODO`, gfx-rs/wgpu#4105), a DX12 odmawia `ResizeBuffers`, dopóki ktokolwiek trzyma bufor zwrotny. Graf tworzył widoki wyjściowej tekstury, a `DeferredRelease` niszczył je dopiero po `frames_in_flight` klatkach, już po `present`, więc żyły w chwili `configure`; `wait_idle` nic tu nie dawał, bo trzymało je CPU, nie GPU. Poprawka w `pathon-rhi-wgpu`: `present_surface`, `discard_surface` i `destroy_surface` zwalniają widok klatki i wszystkie widoki utworzone z jej tekstury (uchwyty u wołającego stają się nieważne, późniejsze `destroy` jest nieszkodliwe). Test `examples/tests/surface_reconfigure.rs` (DX12 i Vulkan) renderuje klatki, rekonfiguruje powierzchnię (te same parametry, kilka zmian rozmiaru okna, kolejne przestrzenie barw) i wymaga, by klatki dalej się rysowały. Ograniczenie: bind groupy, które host sam zbudował z widoku klatki, trzeba zwolnić przed `configure`.
- Brakuje metadanych HDR (`SetHDRMetaData`): wgpu ich nie zapisuje, więc `Hdr10St2084` to „PQ w BT.2020” bez MaxCLL. Dla większości kompozytorów wystarcza; do ewentualnego rozszerzenia przez `interop`.
- Do M4-16 zostaje: tonemapping (krzywa, gamut, kodowanie dla każdej przestrzeni, headroom) w `pathon-post` (M4-09) oraz obraz na prawdziwym monitorze HDR z włączonym HDR w systemie.
