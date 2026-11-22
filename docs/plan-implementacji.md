# pathon — plan implementacji

> Stan na 20.11.2026 (po review własnym i review Fable; dodane hosty: własny silnik, Bevy, Godot) · wgpu 30 · winit 0.30 · Rust 1.97
> Cel: ultra-modularny i wydajny silnik renderowania — mały rdzeń (RHI, graf klatki, rejestr paczek), a każda technika renderowania jest paczką.
> Architektura: dokument „pathon — architektura i implementacja” + canvas „pathon — modularny renderer w Rust”.

## Zasady, które obowiązują w każdym tasku

- Silnik mówi tylko do `pathon-rhi`. wgpu importuje wyłącznie `pathon-rhi-wgpu`.
- Renderer nie zależy od warstwy platformy i nie zakłada jednego okna. Cele renderu (okna, tekstury offscreen) dodaje się do działającego renderera.
- **Core nie wie, kto jest hostem.** `pathon-core` nie ma pętli, okna, wątku, ECS ani systemu assetów. Host (własny silnik, Bevy, Godot) woła klatkę, dostarcza dane przez API extract i może być właścicielem urządzenia oraz celów renderu.
- Nowy host nie może wymagać zmian w `pathon-core`. Jeśli wymaga, poprawiamy API — tak samo jak przy paczkach.
- winit importuje wyłącznie `pathon-platform-winit`. Aplikacja widzi własne typy (`App`, `PlatformEvent`, `WindowId`).
- **Dwa rodzaje paczek.** Zwykła zależy tylko od `pathon-api` i `pathon-shader`. Vendorowa (np. FSR 3 przez sp-fidelity, FSR 4, DLSS) może dodatkowo użyć modułu `interop` konkretnego backendu; deklaruje to w `Cargo.toml` (`[package.metadata.pathon] kind = "vendor"`) i w `requirements()`, a deps-check dopuszcza tylko taki wyjątek.
- Wymagania paczek są znane **przed** utworzeniem urządzenia (backend, funkcje, rozszerzenia), a decyzja pełna ścieżka / fallback / wyłączenie zapada **po** (na prawdziwych `Caps`).
- Nowa paczka zwykła nie może wymagać zmian w `pathon-core`. Jeśli wymaga, poprawiamy `pathon-api`.
- **Źródłem prawdy są bity `Caps`**, nie numer tieru. Profile (Web, Desktop, High) to tylko nazwane zestawy bitów do komunikacji i testów.
- **Web tylko przez WebGPU.** WebGL2 jest poza zakresem (ADR w M0-00): wymagałby drugiego modelu danych w całym rdzeniu (brak compute, indirect i storage bufferów) i nie pozwala utworzyć adaptera bez okna.
- Materiał to funkcja „wejścia powierzchni → BSDF”, niezależna od ścieżki rasteryzacji (forward, visibility buffer, RT).
- Każda decyzja runtime ma diagnostykę z kodem i podpowiedzią. Każda paczka ma przykład w `examples/` i stronę w dokumentacji.
- Każdy task ma kryterium „Gotowe gdy”, które da się sprawdzić testem albo pomiarem.

## Hosty i tryby integracji

> **Bevy i Godot odłożone (ADR-0015).** Kolumny Bevy i Godot w tabeli poniżej oraz etapy M6 i M7 są poza zakresem do odwołania; tryby A/B/C zostają dla własnego silnika.

pathon ma działać we własnym silniku, zamiast renderera Bevy i w Godocie. Generyczność opiera się na trzech możliwościach `pathon-core` — bez traitu `Host`; każdy host to osobny crate, który z nich korzysta.

| Możliwość | Warianty |
| --- | --- |
| Urządzenie | pathon je tworzy (`build()`) **albo** przyjmuje cudze (`with_device(...)` przez `interop`) |
| Klatka | `render_frame()` (pathon zgłasza na kolejkę) **albo** `record_frame()` zwraca listę komend, którą host nagrywa do swojego enkodera (`interop::encode_into`) w punkcie swojej klatki |
| Cele | surface, offscreen **albo** zaimportowana tekstura hosta z deklarowanym `Access` na wejściu i wyjściu |

Tryby integracji — wspólne nazwy dla wszystkich hostów:

| Tryb | Co robi pathon | Własny silnik | Bevy | Godot |
| --- | --- | --- | --- | --- |
| A: paczki w rendererze hosta | graf pathon w punktach klatki hosta, bufory hosta są slotami | tak | paczki M5 na 3D Bevy (opcjonalnie, M6-05) | `CompositorEffect` (M7-04) |
| B: scena do tekstury hosta | cała scena na wspólnym urządzeniu, host składa obraz (np. UI na wierzchu) | tak | UI Bevy nad sceną pathon (M6-04) | `Texture2DRD` (M7-05) |
| C: jedyny renderer | pathon ma urządzenie i surface'y | `pathon-platform` albo własne okno | Bevy bez `bevy_render` (M6-03) | niemożliwe (wymagałoby forka silnika) |

Hosty na wgpu (własny silnik, Bevy) współdzielą jedno `wgpu::Device` z pathon, więc muszą używać tej samej głównej wersji wgpu. Bevy 0.20.1 jest na wgpu 30 — tak jak pathon. Godot ma własny backend RHI (M7-01), bez tego ograniczenia.

## Profile możliwości

| Profil | Gdzie | Co daje ponad poprzedni |
| --- | --- | --- |
| Web | WebGPU w przeglądarce (Chrome, Edge, Safari 26, Firefox) | compute, storage buffery, indirect; bez binding arrays i pipeline cache; jeden wątek |
| Desktop | wgpu natywnie: DX12, Vulkan, Metal | binding arrays (bindless), multi-draw indirect, wiele wątków |
| High | nowsze GPU, funkcje eksperymentalne | 64-bit atomics (Vulkan, DX12, Metal), ray query (w wgpu 30 tylko Vulkan), mesh shadery |
| Vendor | konkretny backend + SDK producenta | FSR 4 (Vulkan + Radeon), DLSS (Vulkan + RTX, poza planem) |

Paczka deklaruje konkretne bity (np. `SHADER_INT64_ATOMIC_MIN_MAX`), nie profil.

## Przegląd etapów

| Etap | Cel | Kryterium ukończenia |
| --- | --- | --- |
| M0 | Decyzje (ADR), szkielet, szew platformy, okno na 4 platformach | `clear` działa na Windows, Linux, macOS i w Chrome; renderer rysuje też w oknie spoza `pathon-platform` |
| M1 | RHI z modelem bindowania, Caps, diagnostyka, drugi backend | test zgodności RHI (tekstura + sampler + compute + raster) zielony na `wgpu` i `null`; `null` odrzuca złe przejścia stanów |
| M2 | Frame graph, kontrakt paczki, pionowy wycinek, szew hosta, spike ash | mesh przechodzi przez graf w 2 oknach + 1 celu offscreen; ten sam wycinek działa w obcym hoście na jego urządzeniu; 4 adwersaryjne paczki testowe bez zmian w rdzeniu; poprawki `rhi-types` ze spike'a |
| M3 | Shadery | zmiana `.wgsl` bez restartu; p99 czasu klatki w normie przy wstrzykiwaniu nowych permutacji (natywnie) |
| M4 | Core działa | pełna klatka PBR + IBL bez paczek technologii, także w Chrome; golden images od początku etapu; test generyczności zielony; baseline wydajności w repo |
| M5 | Pierwsze paczki i scenariusz edytora | TAA, SSAO, FSR 1, SSS, SSR, FXAA, picking bez zmian w `pathon-api` (sprawdzane automatycznie) |
| M6 | Bevy — odłożone (ADR-0015) | scena testowa przez Bevy w trybach C i B przechodzi test generyczności; paczki M5 bez zmian w kodzie |
| M7 | Godot — odłożone (ADR-0015) | paczki z M5 działają w scenie Godota bez zmian w kodzie; renderer pathon rysuje do tekstury w Godocie |
| M8 | High i vendor | meshlety (bez mesh shaderów), RT na Vulkanie, FSR 4 przez interop; ADR o backendzie natywnym |
| M9 | Backendy natywne (warunkowo) | `pathon-rhi-vk` przechodzi test zgodności; paczki zwykłe i hosty bez zmian, vendorowe z nowym `interop` |

## Ścieżka minimalna i linia cięcia

**Ścieżka minimalna do pierwszej pełnej klatki (★):** M0-00…M0-10, M1-01…M1-06, M2-01…M2-06, M2-09…M2-11, M2-18, M2-19, M2-20…M2-24, M3-01…M3-04, M4-00…M4-06, M4-09, M4-11, M4-20.

**Można przesunąć bez szkody dla architektury:** M2-12 profil, M2-13 graph-viz, M3-05 cache na dysku, M3-06 hot-reload, M4-08 niebo, M4-12 skinning, M5-06 SSR, M5-09 picking, M5-11 FSR 3, M6-05 tryb A w Bevy, M7-06 integracja z edytorem Godota, M7-08 RT w Godocie, cały M8 i M9.

Hosty (M6, M7) nie są na ścieżce minimalnej, ale ich szew jest: M2-19 i M4-20 pilnują, że core da się użyć z cudzym urządzeniem, zanim powstanie pierwszy adapter.

---

## M0 — Decyzje, szkielet, szew platformy

- [ ] **M0-00 · Decyzje startowe (ADR)** ★
  - **Stan 20.11.2026:** 8 ADR w docs/adr/; nazwa `pathon` wolna na crates.io, rezerwacja czeka na autora (ADR-0002).
  - Licencja (publikacja, vendoring, dystrybucja bibliotek SDK).
  - Rezerwacja nazwy `pathon` na crates.io z twojego konta.
  - **WebGL2 poza zakresem**, web tylko przez WebGPU.
  - **Model wątków:** `pathon-core` jest `Send` i synchroniczny, nie tworzy wątków. Wątek główny = platforma i surface (wymóg macOS). Wątek renderu (urządzenie i graf) to opcjonalny runner w fasadzie `pathon` (feature `runtime`) dla trybu C we własnym silniku; w Bevy i Godocie wątek daje host. `Device` jest `Send + Sync` natywnie, a na wasm wszystko działa w jednym wątku. Kto robi acquire, present i resize — per tryb integracji.
  - **Hosty i tryby integracji:** tryby A / B / C (tabela na górze), kto jest właścicielem urządzenia, kto steruje klatką, kto zgłasza komendy.
  - **Wejście shaderów do RHI:** źródło WGSL + refleksja z naga; backendy natywne tłumaczą przez naga (SPIR-V, MSL) we własnym zakresie.
  - Logowanie: fasada `log` w rdzeniu, `tracing` tylko za feature w `pathon-profile`.
  - Pula wątków: trait `TaskPool` z domyślną implementacją na `std::thread`, z możliwością podpięcia puli z własnego ECS albo hosta.
  - **Gotowe gdy:** 8 ADR w `docs/adr/`, nazwa zarezerwowana.

- [x] **M0-01 · Workspace Cargo** ★
  - `[workspace.dependencies]` z przypiętym `wgpu = "30"` i `winit = "0.30"`; rustfmt i clippy według mzak-dev Rust style guide.
  - Katalogi: `crates/`, `packs/`, `hosts/` (adaptery Bevy i Godota), `tools/`, `xtask/`, `examples/`, `docs/` (ADR, kody diagnostyk, strony paczek).
  - **Gotowe gdy:** `cargo check --workspace` przechodzi na pustych crate'ach.

- [x] **M0-02 · `xtask deps-check`** ★
  - Reguły: wgpu tylko w `pathon-rhi-wgpu`; winit tylko w `pathon-platform-winit`; L2–L5 nie zależą od `pathon-platform*`; paczka zależy od `pathon-api`, a od `pathon-rhi-*::interop` tylko z `kind = "vendor"`.
  - Hosty: `bevy` tylko w `pathon-bevy`; `godot` tylko w `pathon-rhi-godot` i `pathon-godot`. Rodzaj `kind = "host"` może używać `interop` (jak paczka vendorowa). Nic poza `hosts/` nie zależy od crate'ów hostów.
  - Raport liczby crate'ów rdzenia; wzrost o więcej niż 10 wymaga uzasadnienia w PR.
  - **Gotowe gdy:** każda z reguł ma test, który celowo ją łamie i sprawdza, że CI przerywa z czytelnym komunikatem.

- [x] **M0-03 · CI** ★
  - **Stan 21.11.2026:** pierwszy przebieg na GitHubie zielony (Windows, Linux, macOS, GPU na lavapipe, lint). Zadanie `wasm` buduje przykład `examples/web` w release, lintuje runner webowy i wpisuje rozmiar `.wasm` (surowy i gzip) do podsumowania joba — pierwszy raport w przebiegu 44922768308 (21.11.2026); lokalnie 664 KB / 273 KB.
  - Macierz: Windows, Linux, macOS (Apple Silicon), `wasm32-unknown-unknown`; fmt, clippy, test, deps-check, rozmiar release WASM.
  - **Gotowe gdy:** wszystkie joby zielone na pustym workspace; raport rozmiaru WASM w podsumowaniu joba.

- [x] **M0-04 · `pathon-platform` — kontrakt platformy** ★
  - `WindowId`, `WindowDesc`, `InputEvent`, `PlatformEvent`: Resized, ScaleChanged, RedrawRequested, Input, CloseRequested oraz **`SurfacesAvailable` / `SurfacesLost`** (zamiast polegać na `Resumed` — tak modeluje to winit 0.31).
  - Trait `App { on_event, on_frame }` i trait `Platform { run(app) }`; wiele okien od początku.
  - **Gotowe gdy:** crate kompiluje się bez żadnej biblioteki okienkowej.

- [ ] **M0-05 · `pathon-platform-winit`** ★
  - **Stan 20.11.2026:** działa na Windows (przykłady clear, vertical_slice); Linux i macOS do ręcznego sprawdzenia.
  - Implementacja `Platform` na winit 0.30; tłumaczenie zdarzeń; pętla na wątku głównym.
  - **Gotowe gdy:** okno reaguje na resize i zmianę skali na Windows, Linux (X11 i Wayland) i macOS.

- [ ] **M0-06 · `pathon-platform-web`** ★
  - **Stan 21.11.2026:** komunikat bez WebGPU działa (sprawdzony w przeglądarce w aplikacji), przykład loguje do konsoli i pokazuje pętlę klatek. Powierzchnie webowe renderują przez widok sRGB, więc kolor jest taki sam jak natywnie. Renderowanie w Chrome i Safari 26 z WebGPU czeka na ręczne sprawdzenie.
  - Canvas, asynchroniczny start, `requestAnimationFrame`; brak WebGPU → czytelny komunikat w stronie, nie fallback.
  - **Gotowe gdy:** przykład startuje w Chrome i Safari 26; w przeglądarce bez WebGPU pokazuje komunikat.

- [x] **M0-07 · `pathon-rhi-types` v0** ★
  - Typy bazowe: `Format`, `Usage`, `Extent3d`, `TextureDesc`, `BufferDesc`, `Caps` (bity), `RhiError`, handle'y generacyjne.
  - `SurfaceTarget` (dowolne okno `HasWindowHandle + HasDisplayHandle` + rozmiar + skala) — pasuje też do okien hostów (np. `RawHandleWrapper` w Bevy).
  - `DeviceRequest`: preferowany backend, wymagane / opcjonalne funkcje, `compatible_surface: Option<SurfaceTarget>`, hak rozszerzeń backendu.
  - Minimalne nagrywanie: clear + present (rozszerzane w M1-05).
  - **Gotowe gdy:** crate kompiluje się z zależnościami tylko `bitflags` i `raw-window-handle`.

- [ ] **M0-08 · `pathon-rhi-wgpu`** ★
  - **Stan 20.11.2026:** sprawdzone na Windows (test zgodności na GPU); Metal i WebGPU do sprawdzenia.
  - Urządzenie z `DeviceRequest`, asynchronicznie; konwersje typów makrem z jednej tabeli; brak `impl From<wgpu::X>` w publicznym API.
  - Moduł `interop` (surowe `wgpu::Device`, `Queue`, tekstury, enkoder) — publiczny, ale dostępny tylko paczkom vendorowym i hostom (reguła deps-check).
  - `interop::adopt(instance, adapter, device, queue)` — przyjęcie urządzenia utworzonego poza pathon; `Caps` liczone z przyjętego adaptera. W drugą stronę eksport surowych obiektów, żeby host mógł użyć urządzenia pathon.
  - **Gotowe gdy:** urządzenie powstaje na DX12, Vulkanie, Metalu i WebGPU w CI lub ręcznym teście na sprzęcie; test przyjmuje urządzenie utworzone surowym wgpu i raportuje te same `Caps`.

- [ ] **M0-09 · `pathon-rhi` (fasada)** ★
  - **Stan 20.11.2026:** natywnie sprawdzone; wasm tylko kompiluje się (cargo check).
  - `enum Device { Wgpu(..) }`, `build_async()` + natywny blokujący `build()`, re-eksport typów.
  - **Gotowe gdy:** ten sam kod tworzy urządzenie natywnie (blokująco) i na wasm (async).

- [ ] **M0-10 · Przykład `clear`** ★ — **Gotowe gdy:** spełniona pierwsza część kryterium M0.
  - **Stan 20.11.2026:** działa na Windows; wersja webowa (`examples/web`) buduje się, obraz do sprawdzenia w Chrome; Linux i macOS do sprawdzenia.

- [x] **M0-11 · Przykład `external-window`** — okno z surowego winita albo SDL3, bez `pathon-platform`. **Gotowe gdy:** przykład rysuje.
  - **Stan 20.11.2026:** działa na Windows (surowy winit, bez pathon-platform).

## M1 — RHI z modelem bindowania, Caps, diagnostyka, drugi backend

- [ ] **M1-01 · Caps** ★
  - **Stan 20.11.2026:** Caps z adaptera i matryca formatów gotowe, test konwersji w obie strony; porównanie na 3 systemach i w Chrome do zrobienia.
  - Bity funkcji i limitów z adaptera, matryca formatów tekstur (BCn, ASTC, ETC2, formaty storage), nazwane profile Web / Desktop / High.
  - **Gotowe gdy:** raport `Caps` przy starcie zgadza się z `get_capabilities` / `features()` wgpu na 3 systemach i w Chrome.

- [x] **M1-02 · `pathon-diag`** ★
  - Typ `Diagnostic` w każdym buildzie; renderer w stylu rustc (miette) tylko w dev; w release tekst przez `log`.
  - Kody P / R / G / S / F, strona per kod w `docs/diag/`.
  - **Gotowe gdy:** test sprawdza, że każdy zarejestrowany kod ma stronę w dokumentacji.

- [x] **M1-03 · Device lost** ★
  - Wykrycie, pełna przebudowa urządzenia i surface'ów; callback aplikacji „wgraj dane ponownie”; `on_device_reset` w paczkach (M2-10).
  - Przy urządzeniu pożyczonym od hosta utratę obsługuje host; core daje `reset(new_device)` i ten sam callback ponownego wgrania.
  - **Gotowe gdy:** symulowana utrata urządzenia w teście kończy się działającą klatką po ponownym wgraniu danych — dla urządzenia własnego i pożyczonego.

- [x] **M1-04 · `pathon-rhi-null` z walidacją stanów** ★
  - Wydaje handle'y i nic nie rysuje, ale prowadzi **maszynę stanów per subresource (mip, warstwa)** i odrzuca użycie zasobu w złym stanie `Access`.
  - **Gotowe gdy:** testy celowo psujące przejścia (np. odczyt mipa Hi-Z przed zapisem) kończą się błędem R-kodu.

- [x] **M1-05 · RHI v1: model bindowania i nagrywanie** ★
  - Handle'y: `TextureView`, `Sampler`, `ShaderModule` (WGSL + refleksja), `BindGroupLayout`, `BindGroup`, `RenderPipeline`, `ComputePipeline`.
  - Layouty wierzchołków, bufory vertex / index, viewport, scissor, markery debug.
  - **API nagrywania przez enkoder** (`PassEncoder`: set_pipeline, set_bind_group, draw, draw_indexed, draw_indexed_indirect, dispatch, copy). Czy backend buforuje to w `CommandList`, czy nagrywa od razu — decyzja backendu, niewidoczna dla paczek.
  - `Access` z zakresem subresource (mip, warstwa); `Cmd::External` — callback paczki vendorowej z zadeklarowanymi dostępami, wykonywany przy tłumaczeniu.
  - Timestampy jako atrybut początku i końca passa, nie komenda (na GPU Apple brak timestampów wewnątrz passów).
  - **Gotowe gdy:** M1-06 przechodzi.

- [x] **M1-06 · Test zgodności RHI** ★
  - Kopie bufor↔tekstura, tekstura z samplerem, compute, raster trójkąta z buforem indeksów, timestampy passów; na każdym backendzie.
  - **Gotowe gdy:** zielony na `wgpu` (natywnie) i `null`.

## M2 — Frame graph, kontrakt paczki, pionowy wycinek, szew hosta, spike ash

- [x] **M2-01 · Zasoby grafu** ★
  - Transient, imported, history; każdy zapis tworzy nową wersję.
  - Import tekstury hosta z deklarowanym `Access` przed i po; graf oddaje ją w zadeklarowanym stanie końcowym.
  - **Gotowe gdy:** testy wersjonowania i importu na `null`.
- [x] **M2-02 · Etapy i kolejność** ★ — Stage → before / after → priorytet; kolejność rejestracji jako tie-breaker z ostrzeżeniem. **Gotowe gdy:** test z 3 paczkami na tym samym slocie daje deterministyczną kolejność niezależną od rejestracji.
- [x] **M2-03 · Culling passów** ★ — usuwanie passów bez konsumentów, wyjątek `side_effect`; sloty na żądanie kosztują zero. **Gotowe gdy:** test liczy passy po kompilacji.
- [x] **M2-04 · Czas życia i pule** ★ — interwały życia, reuse obiektów o identycznym opisie. **Gotowe gdy:** test pokazuje mniejszą liczbę alokacji niż zasobów logicznych.
- [x] **M2-05 · Plan barier** ★ — `Access` per subresource zapisywany przez graf; walidowany przez `rhi-null` w każdym teście. **Gotowe gdy:** testy grafu przechodzą pod walidacją stanów.
- [x] **M2-06 · Walidacja grafu** ★ — G0001 cykl, G0003 odczyt bez producenta, G0004 mieszanie render / output size. **Gotowe gdy:** każdy kod ma test.
- [x] **M2-07 · Cache grafu** — rekompilacja tylko przy zmianie paczek, celów, rozmiaru lub profilu. **Gotowe gdy:** licznik kompilacji w teście stabilnej sceny wynosi 1.
  - **Stan 20.11.2026:** `GraphCache` porównuje strukturę grafu dokładnie; test stabilnej sceny: jedna kompilacja na 5 klatek, resize wymusza kolejną. Z cache 5–6× szybciej niż kompilacja ([docs/bench](bench/README.md)).
- [x] **M2-08 · Historia** — `g.history(slot, n)`, kluczowana per widok; pula na maksymalny rozmiar + viewport przy dynamicznej rozdzielczości; unieważnianie przy resize i cięciu kamery. **Gotowe gdy:** testy resize i zmiany render_size.
  - **Stan 21.11.2026:** `history_texture(name, desc)` zwraca parę `previous` / `current` kluczowaną per widok (ADR-0011); resize i zmiana samego `render_size` unieważniają historię (osobne testy w `tests/graph.rs`); `previous_valid` znaczy „to, co ten widok zapisał w ostatniej wysłanej klatce” (kontrakt w `docs/crates/pathon-graph.md`). N = 1 oraz pula na maksymalny rozmiar z viewportem są odłożone (ADR-0011); cięcie kamery to sprawa paczki — czyta `previous_valid`.
- [x] **M2-09 · `pathon-resources`** ★
  - Handle'y generacyjne, opóźnione zwalnianie, staging ring, sub-alokacja buforów.
  - `GpuArray<T>`: tablica danych na GPU (instancje, materiały, kości, światła) jako storage buffer na każdym profilu.
  - **Gotowe gdy:** testy alokatora i zwalniania po N klatkach.
- [x] **M2-10 · `pathon-api`** ★
  - `RenderFeature`: `requirements()` (faza 1: backend, funkcje, rozszerzenia przez hak), `negotiate(&Caps)` (faza 2), **`prepare_view()`** (przed budową grafu), `setup`, `record`, `on_resize`, `on_device_reset`.
  - Rola TemporalResolve w `prepare_view()` ustala `render_size`, sekwencję jittera i mip bias w `ViewCtx` — rdzeń je przechowuje, inne paczki tylko czytają.
  - `ViewCtx` dostaje od hosta czas — core nie czyta zegara. Numer klatki (`frame`) i epokę nagrania (`record_epoch`) liczy core ([ADR-0022](adr/0022-zegar-klatki-paczek.md)).
  - **Kontrakt slotów** (tabela w docs): reverse-Z; wektory ruchu w pikselach, z bieżącej klatki do poprzedniej, bez jittera; normalne oct-encoded w przestrzeni świata; `hdr` liniowe, pre-exposed; slot `exposure`.
  - **Konwencje przestrzeni** w tym samym kontrakcie: układ prawoskrętny, Y w górę, metry, jednostki fizyczne światła (lux, lumen). Konwersje z konwencji hosta robią adaptery hostów, nie paczki.
  - Sloty: `depth`, `depth_pyramid`, `normals`, `motion`, `material_id`, `roughness`, `albedo` (na żądanie — SSR, DLSS RR), `reactive` (maska dla upscalerów), `diffuse` / `specular` (podział światła), `hdr`, `ao`, `exposure`, `output`.
  - **Gotowe gdy:** kontrakt slotów i konwencji opisany i pokryty testem formatów.
- [ ] **M2-11 · `pathon-core`** ★
  - **Stan 20.11.2026:** natywnie z runnerem (wątek pathon-render) działa; na wasm rdzeń kompiluje się, uruchomienie czeka na M0-06.
  - `Renderer::builder()…build_async()` / `build()` — bez okna; `with_device(...)` zamiast tworzenia własnego urządzenia; `add_surface`, `add_offscreen_target`, `import_texture`, `remove_*`, headless.
  - Wejścia klatki: `render_frame()` (pathon zgłasza na własną kolejkę, robi acquire i present swoich surface'ów) oraz `record_frame()` (zwraca `CommandList`; host nagrywa ją do swojego enkodera przez `interop::encode_into` i zgłasza sam).
  - Core nie tworzy wątków; runner z wątkiem renderu zgodny z ADR z M0-00 żyje w fasadzie `pathon` (feature `runtime`).
  - Rejestr paczek, przydział ról (F0102), konflikt backendów (F0201).
  - **Gotowe gdy:** test potwierdza, że core nie tworzy wątków; przykład z runnerem działa natywnie na wątku renderu i w jednym wątku na wasm.
- [ ] **M2-12 · `pathon-profile`** — timestampy passów, `tracing` + Tracy / puffin za feature. **Gotowe gdy:** zrzut z Tracy z nazwanymi passami.
- [x] **M2-13 · `pathon-graph-viz`** — eksport grafu do JSON i DOT (JSON to snapshoty devtools, DOT: `pathon_graph::to_dot` i klawisz `d`). **Gotowe gdy:** DOT renderuje się w Graphviz dla przykładu z M2-18. Sprawdzone 2026-11-21: DOT prawdziwej przechwyconej klatki (backend null, 6 passów) wyrenderowany Graphviz 16.1.2 (`dot -Tsvg`).
- [x] **M2-14 · `pathon-test` i paczki adwersaryjne** ★
  - **Stan 20.11.2026:** zamiast paczki z hookiem materiału jest OnDemandConsumer (sloty na żądanie) — prawdziwe hooki WGSL przyjdą z M4-03.
  - Paczki testowe: podmiana roli Geometry, upscaler zmieniający `render_size`, paczka z hookiem materiału, atrapa paczki vendorowej z `Cmd::External`.
  - **Gotowe gdy:** wszystkie 4 działają bez zmian w `pathon-core` i `pathon-api`.
- [ ] **M2-15 · Spike backendu ash (wyrzucany)**
  - Clear, trójkąt, compute **oraz** tekstura z samplerem, tablica bindless, resize / present, zasób z barierami per mip.
  - Papierowe mapowanie RHI v1 na `RenderingDevice` Godota (bez kodu), żeby backend z M7 nie wymusił przebudowy API na końcu projektu.
  - **Gotowe gdy:** lista poprawek `rhi-types` wniesiona przed M3.
  - **Stan 21.11.2026:** lista zmian wniesiona ([spike-ash.md](spike-ash.md), po przeglądzie), zadania M2-20…M2-24; w prototypie **nie uruchomiono** trójkąta, tekstury z samplerem ani resize / present. Odhaczyć po ich uruchomieniu.
- [x] **M2-16 · Benchmark narzutu nagrywania** — ta sama scena przez API enkodera i bezpośrednio przez wgpu. **Gotowe gdy:** wynik w `docs/bench/`; jeśli narzut przekracza próg ustalony w ADR, backend wgpu przechodzi na nagrywanie bezpośrednie (bez zmian w API paczek).
  - **Stan 20.11.2026:** narzut listy komend +4–6% (próg 15%, ADR-0009) — backend wgpu zostaje przy liście komend ([docs/bench](bench/README.md)).
- [x] **M2-17 · Przykład `multi-view`** — 2 okna + cel offscreen. **Gotowe gdy:** działa natywnie na 3 systemach. *(Uruchomiony na Windows; na Linux/macOS buduje się w CI.)*
- [x] **M2-18 · Pionowy wycinek** ★
  - **Stan 20.11.2026:** test GPU (offscreen) sprawdza piksele; przykład vertical_slice z dwoma oknami uruchomiony na Windows.
  - Jeden mesh unlit z zaszytym shaderem WGSL przechodzi przez cały stos: platforma → core → graf → RHI, w 2 oknach.
  - **Gotowe gdy:** mesh widać w obu oknach; to pierwsza prawdziwa geometria w projekcie.
- [x] **M2-19 · Szew hosta** ★
  - **Stan 20.11.2026:** examples/tests/host_seam.rs: tryb B identyczny piksel w piksel z trybem C, tryb A rysuje na obrazie hosta.
  - Przykład `host-foreign-device`: surowa aplikacja wgpu + winit sama tworzy urządzenie, okno i pętlę, a pathon je przyjmuje (`with_device`).
  - Tryb B: wycinek z M2-18 renderuje się do tekstury hosta (`import_texture`), host ją wyświetla.
  - Tryb A: paczka testowa z M2-14 działa jako post-process na teksturze hosta przez `record_frame` + `interop::encode_into`.
  - **Gotowe gdy:** obraz w trybie B jest identyczny z trybem C na tym samym backendzie, a w `pathon-core` nie przybyła żadna zależność.
- [x] **M2-20 · Kontrakty `rhi-types` + ADR** ★ — dokument i ADR z decyzjami ze [spike-ash.md](spike-ash.md) #6: poprzedni stan w `TextureAccess`, wątki (które metody `DeviceApi` poza wątkiem renderu), konwencja współrzędnych, polityka rozrostu struktur, poprawione `bytes_per_row`. **Gotowe gdy:** ADR i `docs/crates/pathon-rhi-types.md` istnieją, a test konformancji `rhi-null` i `rhi-wgpu` sprawdza stan zachowany po `write_*` i `SetBindGroup` przed `Set*Pipeline`.
  - **Stan 21.11.2026:** ADR-0020 przyjęty; `TextureAccess { from, to }`, `Access: u16`, `COPY_DST` po `write_texture`, R0113 i test konformancji gotowe. Polityka rozrostu struktur (`#[non_exhaustive]`, konstruktory) jest tylko decyzją — wejdzie przed M5-07. Do zrobienia osobno: `rhi-null` nie sprawdza jeszcze, czy dane / bufor wystarczają na układ ani wyrównania offsetu bufora (wgpu sprawdza).
- [x] **M2-21 · Podział `Access::SHADER_READ`** ★ — `SAMPLED`, `STORAGE_READ`, `STORAGE_WRITE`; poprawka `usage_for` w `pathon-graph` (`STORAGE_READ` → `TextureUsage::STORAGE`), ostrzejszy `is_valid_state`, walidacja w `rhi-null`. **Gotowe gdy:** test grafu, w którym pass czyta teksturę storage bez zapisującego passa, dostaje `STORAGE`, a `SAMPLED\|COPY_SRC` i `SAMPLED\|STORAGE_READ` są odrzucone.
- [x] **M2-22 · Tablice bindingów w RHI** ★ — `count` w `BindGroupLayoutEntry`, `TextureViewArray` / `SamplerArray`, limit samplerów, reguła „bez uniform buffera w grupie z tablicą”, decyzja o modelu aktualizacji. **Gotowe gdy:** test `rhi-wgpu` na GPU czyta element tablicy 1024 tekstur, a `rhi-null` odrzuca grupę z tablicą i uniform bufferem.
- [x] **M2-23 · Immediates i overrides** ★ — `Features::IMMEDIATES`, `immediate_size` w opisach potoków, `Command::SetImmediates` mieszczący się w 32 B, nazwane overrides `f64` rozwiązywane w naga. **Gotowe gdy:** test GPU przekazuje wartość przez immediate i przez override, a rozmiar `Command` pozostaje ≤ 32 B (test).
- [x] **M2-24 · Pipeline cache i źródło shaderów** ★ — blob cache w `DeviceRequest` + getter; ADR: WGSL czy SPIR-V w `create_shader_module`; lista pól klucza potoku (bias, stencil, polygon mode, alpha-to-coverage). **Gotowe gdy:** test round-trip blobu na wgpu/Vulkanie i ADR z decyzją przed M3-01.
  - **Stan 21.11.2026:** ADR-0019 przyjęty; `DeviceRequest::pipeline_cache`, `DeviceApi::pipeline_cache_data` i cache w `rhi-wgpu` gotowe, round-trip na Vulkanie przechodzi (`crates/pathon-rhi/tests/pipeline_cache.rs`). Pola klucza potoku (bias, stencil, polygon mode, alpha-to-coverage) to tylko decyzja — dojdą z pierwszym użytkownikiem.

## M3 — Shadery

- [x] **M3-01 · `pathon-shader` + importy** ★ — WGSL z wesl 0.6 (ADR-0021; dokłada 50 crate'ów na Windows): `ShaderComposer` składa moduły `package::…` i oddaje źródło WGSL; zależność wpięta w `pathon-shader` przez xtask. **Gotowe gdy:** moduł z importami kompiluje się i trafia do RHI.
- [x] **M3-02 · Refleksja** ★ — layouty bind groupów z naga. **Gotowe gdy:** layout z refleksji równa się ręcznie zapisanemu w teście.
  - **Stan 22.11.2026:** `reflect_bind_groups` w `pathon-shader` (naga 30, tylko `wgsl-in`, wpięta w `pathon-shader` przez xtask) zwraca layouty grup z `visibility` z podanych entry pointów; test porównuje wynik z ręcznie zapisanym layoutem shadera render, a test GPU tworzy z niego potok compute.
- [x] **M3-03 · Permutacje** ★
  - Klucz shadera (flagi materiału, hooki paczek, profil) oddzielony od **klucza pipeline'u** (layout wierzchołków, formaty celów, MSAA, rodzaj widoku).
  - **Gotowe gdy:** testy kolizji kluczy.
  - **Stan 22.11.2026:** flagi WESL `@if` (`ShaderFeatures`, `compose_with`), `ShaderKey` i `ShaderLibrary` (kesz modułów) oraz `PipelineCache` (kesz potoków po normalizacji `constants`) w `pathon-shader` ([ADR-0023](adr/0023-permutacje-shaderow.md)); xtask dopuszcza `pathon-shader` w paczkach; testy kolizji na `rhi-null`, test GPU z czterema permutacjami.
- [ ] **M3-04 · Kompilacja w tle** ★ — placeholder albo pominięcie draw. **Gotowe gdy:** test wstrzykujący N nowych permutacji trzyma p99 czasu klatki w budżecie (natywnie; na wasm tylko pomiar).
- [ ] **M3-05 · Cache na dysku** — `PipelineCache` na Vulkanie. **Gotowe gdy:** drugi start skraca czas kompilacji (pomiar).
- [ ] **M3-06 · `pathon-hot`** — notify, podmiana modułu. **Gotowe gdy:** zmiana `.wgsl` widoczna bez restartu.
- [ ] **M3-07 · Błędy shaderów** — kody S z lokalizacją. **Gotowe gdy:** celowy błąd w `.wgsl` pokazuje plik, linię i podpowiedź.

## M4 — Core działa

- [ ] **M4-00 · Golden images** ★ — **pierwszy task etapu.** Render headless; referencje per backend albo metryka percepcyjna (np. FLIP); CI na lavapipe, WARP i macOS (sprawdzić Metal na hostowanych runnerach, w razie potrzeby własny runner). **Gotowe gdy:** golden test wycinka z M2-18 w CI.
- [ ] **M4-01 · `pathon-mesh`** ★
  - Pula geometrii (mega-bufory vertex / index), layouty wierzchołków, upload meshy, dane poprzedniej klatki (transformacje, pozycje skinowane) dla wektorów ruchu.
  - Wspólna dla forward, visibility buffer i RT.
  - **Gotowe gdy:** scena testowa z wieloma meshami w jednym buforze.
- [ ] **M4-02 · `pathon-scene`** ★
  - Render world w SoA, kamery, światła, API extract, podwójny bufor.
  - Bez systemu assetów i I/O plików: dane z pamięci przez handle'y; adaptery hostów mapują na nie swoje assety.
  - **Gotowe gdy:** extract 10 000 instancji mieści się w budżecie z M4-17.
- [ ] **M4-03 · `pathon-material`** ★
  - Materiał jako funkcja WGSL „wejścia powierzchni → BSDF”, oddzielona od entry pointu; ta sama funkcja dla forward, visibility buffer i punktu trafienia RT (bez pochodnych ekranowych).
  - Materiał standardowy: model glTF metallic-roughness + rozszerzenia KHR — wspólny mianownik `StandardMaterial` z Bevy i `StandardMaterial3D` z Godota.
  - Flagi: opaque, masked, transparent, subsurface, transmission; hook podziału `diffuse` / `specular`; tablica materiałów w `GpuArray`.
  - **Gotowe gdy:** ten sam materiał renderuje się w forward i w testowym compute ewaluującym punkt.
- [ ] **M4-04 · Bindless** ★ — profil Desktop: `binding_array`; profil Web: bind group per materiał, ten sam indeks logiczny. **Gotowe gdy:** ta sama scena na obu profilach.
  - **Stan 22.11.2026:** warunek wstępny RHI spełniony (PAT-119): `Features::NON_UNIFORM_INDEXING` z `TEXTURE_BINDING_ARRAY` (profil Desktop) ma test GPU, w którym każdy wątek compute i każdy piksel czyta inny element `binding_array`.
- [ ] **M4-05 · `pathon-pbr`** ★ — prepass (`depth`, `normals`, `motion`, `material_id`, na żądanie `roughness`, `albedo`, `depth_pyramid`), clustered forward+ (przypisanie świateł w compute, także na Web). **Gotowe gdy:** golden image sceny testowej.
- [ ] **M4-06 · `pathon-ibl`** ★ — environment map, prefiltrowanie, LUT BRDF; bez tego metale są czarne. Przed startem: `view_formats` i zgodność z cube w `TextureDesc` ([spike-ash.md](spike-ash.md) #4; termin twardy M5-07). **Gotowe gdy:** golden image z metalami.
  - **Stan 22.11.2026:** warunek wstępny RHI spełniony: `TextureDesc::view_formats` (bliźniak sRGB) i widoki cube / tablic z walidacją w `rhi-null` oraz testem GPU (sześć ścian renderowanych osobno, odczyt `texture_cube`; widok sRGB nad teksturą UNORM); aneks w [ADR-0020](adr/0020-kontrakty-rhi-types.md). `GraphTextureDesc` dalej nie ma cube'a.
- [ ] **M4-07 · `pathon-shadow`** — CSM + PCF, rola Shadows. **Gotowe gdy:** golden image z cieniami.
- [ ] **M4-08 · `pathon-sky`** — atmosfera; jako źródło dla IBL. **Gotowe gdy:** golden image.
- [ ] **M4-09 · `pathon-post`** ★ — ekspozycja (slot `exposure`), bloom, tonemapping SDR. **Gotowe gdy:** golden image.
- [ ] **M4-10 · Scena testowa** — loader glTF tylko w `examples/`. **Gotowe gdy:** scena ładuje się na 3 systemach i w Chrome.
- [ ] **M4-11 · Culling** ★ — frustum na CPU; na Desktop culling na GPU + indirect (`MULTI_DRAW_INDIRECT_COUNT` na DX12 i Vulkanie; na Metalu wariant bez count). Dołożyć komendy indirect (`DrawIndirect`, `DispatchIndirect`, `MultiDraw…`, `…Count`; limit 32 B, powtórzyć benchmark ADR-0009), patrz [spike-ash.md](spike-ash.md). **Gotowe gdy:** liczba draw calli spada w pomiarze, golden images bez zmian.
- [ ] **M4-12 · `pathon-skin`** — skinning w compute, morph targets, pozycje poprzedniej klatki dla wektorów ruchu (layout ustalony w M4-01). **Gotowe gdy:** animowany model z poprawnym TAA (bez smużenia) w M5.
- [ ] **M4-13 · Tekstury** — kompresja według matrycy `Caps` (iOS bez BCn), mipmapy w compute, opcjonalny `pathon-ktx2`. **Gotowe gdy:** ta sama scena z teksturami BCn na desktopie i fallbackiem tam, gdzie BCn brak.
- [ ] **M4-14 · `pathon-debug`** — linie, gizmo, AABB, wireframe, podgląd slotów. **Gotowe gdy:** podgląd każdego slotu bazowego w przykładzie.
- [ ] **M4-15 · Obiekty przezroczyste i transmission** — sortowanie od tyłu; kopia koloru opaque dla materiałów z `transmission`; OIT jako przyszła paczka. **Gotowe gdy:** golden image ze szkłem.
- [ ] **M4-16 · Wyjście HDR** — `get_capabilities` decyduje o dostępnych `SurfaceColorSpace` (`Auto` nigdy nie wybiera HDR, wgpu nie robi tone mappingu); tonemapping pod HDR w `pathon-post`, fallback SDR. **Gotowe gdy:** obraz HDR na monitorze HDR i poprawny SDR bez niego.
- [ ] **M4-17 · Budżety**
  - Lokalny `xtask bench`: scena testowa w 1440p na RX 9070 XT i MacBooku Air M5, baseline w repo, alarm przy regresji w PR (uruchamiany ręcznie).
  - W CI: rozmiar release WASM i czasy CPU na `rhi-null`.
  - **Gotowe gdy:** baseline zapisany, próg regresji w ADR.
- [ ] **M4-18 · Przykład integracji z ECS** — scena z prostego `Vec` encji + adapter extract; wzór dla adapterów hostów (M6-02, M7-05). **Gotowe gdy:** przykład działa i ma stronę w docs.
- [ ] **M4-19 · MSAA — decyzja** — ADR: czy forward+ wspiera MSAA obok TAA, czy MSAA wypada. **Gotowe gdy:** ADR i, jeśli MSAA zostaje, golden image.
- [ ] **M4-20 · Test generyczności** ★ — ta sama scena testowa i zestaw paczek w trybie C (standalone) i w hoście z M2-19 (tryb B, cudze urządzenie). Każdy kolejny host (M6, M7) dopina się do tego testu. **Gotowe gdy:** oba warianty dają golden image w progu, w CI.

## M5 — Pierwsze paczki i scenariusz edytora

TAA najpierw; jitter i `render_size` ustala rola TemporalResolve w `prepare_view()`.

- [ ] **M5-01 · `pathon-taa`** (rola TemporalResolve) — jitter, historia, clamping w YCoCg. **Gotowe gdy:** golden image ruchomej sceny bez smużenia.
- [ ] **M5-02 · `pathon-ssao`** — GTAO w compute (wszystkie profile). **Gotowe gdy:** golden image.
- [ ] **M5-03 · `pathon-fsr` · FSR 1** — EASU + RCAS we własnym WGSL, zero zależności, także na webie. **Gotowe gdy:** golden image przy 67% rozdzielczości.
- [ ] **M5-04 · `pathon-sss`** — separable SSS na slocie `diffuse`, maska z `material_id`. **Gotowe gdy:** golden image skóry.
- [ ] **M5-05 · `pathon-fxaa`** — AA bez historii. **Gotowe gdy:** golden image.
- [ ] **M5-06 · `pathon-ssr`** — hi-Z trace na `depth_pyramid` + `roughness`. **Gotowe gdy:** golden image.
- [ ] **M5-07 · Test modularności** — `cargo-semver-checks` na `pathon-api` (albo porównanie z zamrożonym tagiem) w CI. **Gotowe gdy:** paczki M5 przeszły bez zmian w API.
- [ ] **M5-08 · Konflikt ról** — TAA i FSR zarejestrowane naraz. **Gotowe gdy:** aktywna jest jedna, druga dostaje F0102.
- [ ] **M5-09 · `pathon-picking`** — bufor ID, odczyt pod kursorem. **Gotowe gdy:** przykład edytorowy zaznacza obiekt.
- [ ] **M5-10 · Dokumentacja paczek** — strona i przykład dla każdej paczki z M4–M5. **Gotowe gdy:** test linków i przykładów w CI.
- [ ] **M5-11 · `pathon-fsr` · FSR 3 (opcjonalne, vendorowe)**
  - Przez `sp-fidelity-wgpu` 0.3 (MIT, wgpu 30, +2 crate'y, upscaling FSR 2 i 3.1.4), z użyciem `interop`.
  - Wymaga `SUBGROUP`, `TEXTURE_ATOMIC`, `TEXTURE_FORMAT_16BIT_NORM`, `FLOAT32_FILTERABLE` — zgłaszane jako opcjonalne w `requirements()`; brak → fallback do FSR 1. Nie działa w przeglądarce.
  - Port jest młody (wiele wydań od października 2026, napisany przez agentów AI) — plan B: fork.
  - **Gotowe gdy:** golden image na Windows i macOS; fallback do FSR 1 przetestowany.

## M6 — Bevy (odłożone, ADR-0015)

Zależy od M4 (tryby C i B) i M5 (tryb A). Idzie przed Godotem, bo Bevy 0.20 jest na wgpu 30: nie potrzebuje nowego backendu RHI, więc najtaniej sprawdza generyczność core'a.

Fakty z 20.11.2026: `bevy_render` 0.20.1 zależy od wgpu 30; `RenderCreation::Manual(Box<RenderResources>)` przyjmuje urządzenie, kolejkę, adapter i instancję utworzone na zewnątrz; okna Bevy mają `RawHandleWrapper`.

- [ ] **M6-00 · Decyzje (ADR)**
  - Jedna wspierana wersja Bevy na wydanie pathon (start: 0.20.x). Tryby A i B wymagają tej samej głównej wersji wgpu, więc pathon podąża za wersją wgpu z Bevy.
  - Kolejność: tryb C, potem B, A opcjonalnie.
  - Pomiar zależności `pathon-bevy` (Bevy z wybranymi features).
  - **Gotowe gdy:** ADR w `docs/adr/`.

- [ ] **M6-01 · Spike (wyrzucany)**
  - Potwierdzić w Bevy 0.20: `RenderCreation::Manual`, `RawHandleWrapper`, punkt wpięcia w render world Bevy (gdzie wołać `render_frame` / `record_frame`).
  - Które typy istnieją bez `bevy_render`: `Mesh` i `Image` prawdopodobnie tak; `StandardMaterial` jest w `bevy_pbr`, który zależy od `bevy_render`, więc w trybie C prawdopodobnie potrzebny własny komponent materiału.
  - **Gotowe gdy:** lista ustaleń wniesiona do M6-02…M6-05.

- [ ] **M6-02 · `pathon-bevy`**
  - `PathonPlugin`; extract `Mesh3d`, `GlobalTransform`, `Camera3d`, światła i materiał do `pathon-scene`; mapowanie `Assets<Mesh>` / `Assets<Image>` na handle'y pathon ze śledzeniem zmian; konwersja jednostek światła; diagnostyka do logów Bevy.
  - Wzór: przykład ECS z M4-18.
  - **Gotowe gdy:** test extractu porównuje zawartość `pathon-scene` z encjami świata Bevy.

- [ ] **M6-03 · Tryb C — Bevy bez swojego renderera**
  - Bevy bez `RenderPlugin`; surface'y z okien Bevy przez `RawHandleWrapper`; pathon tworzy urządzenie i woła `render_frame`.
  - Koszt: nie rysują się UI, sprite'y, tekst ani gizmo Bevy (żyją w `bevy_render`) — kto ich potrzebuje, bierze tryb B.
  - **Gotowe gdy:** scena testowa z M4-10 przez Bevy przechodzi test generyczności z M4-20.

- [ ] **M6-04 · Tryb B — scena pathon pod UI Bevy**
  - pathon tworzy urządzenie z wymaganiami paczek (faza 1 `requirements()`) i przekazuje je Bevy przez `RenderCreation::Manual`.
  - Scena pathon trafia do tekstury widocznej w Bevy jako obraz; Bevy rysuje nad nią UI. Kolejność gwarantuje ta sama kolejka.
  - **Gotowe gdy:** przykład z UI Bevy nad sceną pathon; test potwierdza jedno `wgpu::Device` w procesie; scena przechodzi test generyczności.

- [ ] **M6-05 · Tryb A — paczki pathon na 3D Bevy (opcjonalny)**
  - Prepassy Bevy (głębia, normalne, wektory ruchu) jako zaimportowane sloty; konwersja do kontraktu slotów w passach adaptera, nie w paczkach.
  - **Gotowe gdy:** SSAO i FSR 1 z M5 działają na scenie Bevy bez zmian w kodzie paczek.

- [ ] **M6-06 · Dystrybucja** — crate `pathon-bevy` na crates.io, przykłady trybów C i B, tabela zgodności pathon ↔ Bevy ↔ wgpu w README. **Gotowe gdy:** czysty projekt Bevy z crates.io renderuje scenę testową.

## M7 — Godot (odłożone, ADR-0015)

Zależy od M5 (paczki) i szwu hosta (M2-19), nie od M6, M8 ani M9. Idzie po Bevy, bo wymaga trzeciego backendu RHI.

Godot 4 renderuje przez `RenderingDevice` (Vulkan, D3D12, Metal), a GDExtension daje do niego pełny dostęp. Dlatego integracja to **trzeci backend RHI**, a nie osobny renderer: `pathon-rhi-godot` implementuje `DeviceApi` na urządzeniu Godota, a paczki i graf zostają bez zmian. Podmiany całego renderera Godota GDExtension nie przewiduje (to byłby fork silnika), więc tryb C odpada i zostają dwa tryby z tabeli na górze:

- **Tryb A — paczki pathon w renderze Godota.** `CompositorEffect` wpina graf pathon w punkty klatki Godota (po prepassie, po opaque, po niebie, po przezroczystych). Bufory Godota (kolor, głębia, wektory ruchu, normalne + roughness) stają się slotami pathon.
- **Tryb B — renderer pathon jako węzeł.** Węzeł `PathonView` renderuje scenę pathon do tekstury na tym samym urządzeniu (bez kopii) i pokazuje ją przez `Texture2DRD`.

- [ ] **M7-00 · Decyzje (ADR)**
  - Minimalna wersja Godota (`CompositorEffect` od 4.3, `Texture2DRD` od 4.2) i wersja godot-rust (gdext 0.5, Godot 4.2+).
  - Licencja: gdext jest na MPL-2.0 — dotyczy tylko crate'u rozszerzenia; sprawdzić zgodność z licencją z M0-00.
  - Zakres: najpierw tryb A, potem B; bez weba (w przeglądarce Godot używa renderera Compatibility, w którym `RenderingDevice` nie istnieje).
  - Pomiar zależności gdext.
  - **Gotowe gdy:** ADR w `docs/adr/`.

- [ ] **M7-01 · `pathon-rhi-godot`**
  - `DeviceApi` na globalnym `RenderingDevice`: tekstury, bufory, samplery, uniform sety, compute i draw listy, timestampy.
  - Shadery: WGSL → SPIR-V przez naga (`shader_create_from_spirv`); refleksja z naga jak w pozostałych backendach.
  - `Caps` z `limit_get` i `has_feature`; RD sam śledzi stany zasobów, więc plan `Access` jest tu (jak w wgpu) tylko walidowany.
  - deps-check: biblioteka `godot` tylko w `pathon-rhi-godot` i `pathon-godot`.
  - **Gotowe gdy:** test zgodności RHI zielony w uruchomionym Godocie (RD nie działa headless, więc test idzie przez scenę testową odpalaną z `xtask`, lokalnie albo na własnym runnerze).

- [ ] **M7-02 · `pathon-godot` (GDExtension)**
  - Crate `cdylib` na gdext: rejestracja klas, paczki włączane features jak w fasadzie, diagnostyka `pathon-diag` do konsoli Godota (ostrzeżenia z kodem i podpowiedzią).
  - Wątki: callback `CompositorEffect` przychodzi na wątku renderu Godota — core i tak nie tworzy wątków (ADR z M0-00), więc pathon woła tam `record_frame`.
  - **Gotowe gdy:** pusty efekt ładuje się w edytorze Godota na Windows, Linux i macOS.

- [ ] **M7-03 · Adapter slotów**
  - Bufory Godota → sloty pathon z konwersją do kontraktu slotów (konwencja wektorów ruchu, kodowanie normalnych, ekspozycja, jednostki światła) w passach adaptera, nie w paczkach.
  - Mapowanie `Stage` → punkt callbacku `CompositorEffect`.
  - **Gotowe gdy:** test formatów slotów z M2-10 przechodzi na danych z Godota.

- [ ] **M7-04 · `PathonEffect` — tryb A**
  - Zasób `PathonEffect` dodawany do `Compositor` w `WorldEnvironment` lub `Camera3D`; parametry paczek w inspektorze.
  - Pierwsze paczki: SSAO, SSS, FSR 1; potem TAA (wymaga jittera kamery Godota — sprawdzić, co da się ustawić z rozszerzenia).
  - **Gotowe gdy:** te same crate'y paczek co w M5, bez zmian w kodzie, działają w scenie Godota; golden image na RX 9070 XT i MacBooku M5.

- [ ] **M7-05 · `PathonView` — tryb B**
  - Węzeł renderujący scenę pathon do `Texture2DRD` na urządzeniu Godota.
  - Adapter extract z drzewa sceny (`MeshInstance3D`, światła, `Camera3D`, `StandardMaterial3D` → materiał standardowy z M4-03) do `pathon-scene` — na wzór przykładu ECS z M4-18 i `pathon-bevy` z M6-02.
  - **Gotowe gdy:** scena testowa z M4-10 renderuje się w Godocie przez pathon i przechodzi test generyczności z M4-20 (w progu metryki percepcyjnej, bo backend jest inny).

- [ ] **M7-06 · Integracja z edytorem (opcjonalna)** — podgląd slotów (`pathon-debug`) i panel z grafem (`pathon-graph-viz`) w edytorze. **Gotowe gdy:** podgląd `ao` i `motion` w viewporcie edytora.

- [ ] **M7-07 · Dystrybucja** — plik `.gdextension`, buildy na 3 systemy w CI, przykładowy projekt Godota, wpis w Asset Library. **Gotowe gdy:** czysta instalacja z paczki działa w nowym projekcie.

- [ ] **M7-08 · RT w Godocie (opcjonalnie)** — Godot rozwija ray tracing Vulkan w `RenderingDevice` (BLAS / TLAS, pipeline'y RT); gdy będzie stabilny, `pathon-rt` może z niego korzystać przez `pathon-rhi-godot`. **Gotowe gdy:** decyzja w ADR po sprawdzeniu API.

## M8 — High i vendor

- [ ] **M8-01 · `pathon-fsr4`** (vendorowa)
  - Oficjalne FSR SDK 2.4 (FSR Upscaling 4.1.2) przez surowe urządzenie Vulkan z `interop` (`as_hal`); bez własnego backendu.
  - Wymaga backendu Vulkan na Windows (`requirements()`), GPU Radeon RX 6000+ — testowalne na RX 9070 XT.
  - Zaczyna się od spike'u weryfikującego API SDK (wsparcie Vulkana znam tylko z ogłoszeń).
  - **Gotowe gdy:** FSR 4 działa w przykładzie na RX 9070 XT.
- [ ] **M8-02 · `pathon-meshlet`** (rola Geometry)
  - **Bez mesh shaderów:** indirect + rasteryzacja w compute do visibility buffera na 64-bit atomics min/max (Vulkan, DX12, Metal); materiał z M4-03.
  - Offline'owy builder klastrów i format assetu (`pathon-meshlet-build`) — osobny task w środku.
  - Culling okluzji przez `depth_pyramid`. Mesh shadery jako późniejsza optymalizacja.
  - **Gotowe gdy:** scena z milionami trójkątów na RX 9070 XT i MacBooku M5.
- [ ] **M8-03 · `pathon-rt`** — ray query: cienie i GI w stylu ReSTIR; w wgpu 30 tylko Vulkan, więc wymaga backendu Vulkan; na macOS paczka wyłączona z diagnostyką. **Gotowe gdy:** golden image na Vulkanie (RX 9070 XT).
- [ ] **M8-04 · Raport sufitu wgpu** — async compute, VRAM bez aliasingu, brak RT pipeline'ów. **Gotowe gdy:** ADR o M9.

## M9 — Backendy natywne (tylko jeśli M8-04 tak zdecyduje)

- [ ] **M9-01 · `pathon-rhi-vk`** (ash) — `DeviceApi`, barriery synchronization2 z planu `Access`, aliasing, async compute, descriptor indexing; start od wniosków ze spike'a M2-15.
- [ ] **M9-02 · Wybór backendu w runtime** — Vulkan na Windows / Linux, wgpu na macOS i web.
- [ ] **M9-03 · Zgodność** — test zgodności RHI zielony, golden images zgodne z backendem wgpu.
- [ ] **M9-04 · `interop` dla Vulkana** — paczki vendorowe (FSR 3, FSR 4) dostają surowe urządzenie z nowego backendu.
- [ ] **M9-05 · `pathon-rhi-metal`** (objc2-metal) — po sprawdzonym Vulkanie.
- [ ] **M9-06 · Kolejne backendy platformy (opcjonalnie)** — `pathon-platform-sdl3` albo `-win32`.
  - **Gotowe gdy (M9):** paczki zwykłe i hosty bez zmian; vendorowe działają przez nowy `interop`. Tryby A i B w Bevy zostają na backendzie wgpu (Bevy nie przyjmie urządzenia ash); tryb C w Bevy może użyć backendu natywnego.

---

## Budżet zależności

Zmierzone `cargo tree -e normal,build` (unikalne crate'y), 20.11.2026 (wiersz wesl: Windows, 22.11.2026, wtedy `pathon` miał 95 crate'ów, bez `pathon-shader`):

| Konfiguracja | Windows | Linux | macOS | Web |
| --- | --- | --- | --- | --- |
| pathon dziś (wgpu + winit + bitflags + bytemuck + glam) | 88 | 119 | 81 | 55 |
| + wesl (`pathon-shader`, zmierzone 22.11.2026) | +50 | | | |
| + sp-fidelity-wgpu (FSR 3, opcjonalne) | +2 | +2 | +2 | |
| + narzędzia dev (miette, notify) | +42 | +48 | +49 | +49 |
| + `pathon-bevy` (Bevy 0.20 z wybranymi features) | mierzymy w M6-00 | | | |
| Bevy 0.20 domyślnie (dla porównania) | 374 | 403 | 381 | 355 |

Sam renderer (bez `pathon-platform-winit`) nie ciągnie winita. Nowe zależności z M0-00 (`log`, pula wątków) mierzymy po decyzji. Zależności hostów (Bevy, gdext) dotyczą tylko crate'ów w `hosts/`, nie rdzenia.

## Otwarte decyzje

- [ ] Format serializacji grafu dla edytora: JSON, RON czy binarny.
- [ ] Konkretny próg uruchomienia M9.
- [ ] OIT: czy i kiedy.

## Backlog (poza planem do czasu sprzętu lub potrzeby)

- **`pathon-dlss`** — `dlss_wgpu` 6 jako paczka vendorowa (Vulkan + RTX, feature `mock` na CI). Nie da się przetestować na RX 9070 XT ani na MacBooku M5; wraca, gdy będzie dostęp do karty RTX.
- Mesh shadery jako optymalizacja `pathon-meshlet`.
- FSR / DLSS frame generation.

## Poza zakresem

WebGL2 (decyzja w M0-00), własny ECS (renderer jest ECS-agnostyczny), fizyka, audio, mapowanie inputu, hot-reload kodu Rust, konsole, paczki jako biblioteki dynamiczne, Android i iOS przed M9, rozszerzenie Godota na web (renderer Compatibility nie ma `RenderingDevice`), tryb C w Godocie (wymagałby forka silnika), wsparcie kilku wersji Bevy naraz, paczki vendorowe w Godocie.
