# Filament jako wzór: inwentarz technik, przewagi pathon i kolejność prac

Stan na 25.11.2026. Opis oparty na płytkiej kopii Filamenta (gałąź `main`, commit `863e7d72420e99f4efbed8cafa6a343f3407018d`, nagłówek v1.81.1 w RELEASE_NOTES.md) czytanej poza repo pathon. Ścieżki w tabelach są względem korzenia repozytorium Filamenta; numer po dwukropku to linia w tej rewizji. Wszystko tu to dokumentacja i propozycje: plan ([plan-implementacji.md](plan-implementacji.md)) i Linear aktualizuje orkiestrator.

Zasady użycia kodu Filamenta (Apache-2.0, nagłówki, THIRD_PARTY_NOTICES, które katalogi są poza zezwoleniem): [ADR-0047](adr/0047-uzywanie-kodu-filamenta.md). Skrót: port wymaga nagłówka i wpisu w [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md), a pliki pochodne zostają na Apache-2.0.

## Najważniejsze wnioski

1. **Filament nie ma tego, co najmocniej buduje wygląd wnętrz.** Brak globalnego oświetlenia i lightmap (docs: „Static lighting [TODO]”), brak świateł powierzchniowych (docs: „Area lights [TODO]”), brak lokalnych sond odbić (jedno globalne IBL, bez korekcji paralaksy), brak wolumetryki (tylko analityczna mgła wysokościowa), brak automatycznej ekspozycji w kodzie (sekcja w docs bez implementacji), brak profili IES. To są miejsca, w których pathon może być „nawet lepszy”.
2. **Tam, gdzie Filament jest silny, najpierw osiągamy parytet.** Ma dojrzałe TAA (taa.mat, 795 linii, z osobnym dokumentem projektowym), GTAO z bitmaskami, bent normals, multi-bounce i specular occlusion z stożków, PCSS na EVSM, froxele, SSR, refrakcję z dyspersją i geometryczne specular AA. „Bent-normal specular occlusion” i „GTAO z multi-bounce” **nie są przewagą nad Filamentem** (on je ma), tylko elementem parytetu.
3. **pathon ma już rdzeń BRDF i IBL.** `pathon-material` ma GGX, Smith height-correlated, Schlick, Lambert/Burley, clearcoat, transmission i volume; `pathon-ibl` ma prefiltr GGX, irradiancję, LUT i kompensację wielokrotnego rozpraszania; `pathon-post` ma histogramową auto-ekspozycję, bloom i tonemapping (ACES, PBR Neutral). Do reimplementacji z publikacji nie potrzeba Filamenta.
4. **Największe luki planu dla wnętrz** (nie ma ich w M4-05, M4-07, M5-02 w obecnym brzmieniu): cienie świateł punktowych i spotów w atlasie (M4-07 to CSM + PCF), światła powierzchniowe (`LightKind` ma tylko Directional, Point, Spot), okluzja spekularna z bent normals (slot `ao` to `R8Unorm`, bent normals wymagają slotu deklarowanego przez paczkę), lokalne sondy odbić i jakiekolwiek GI.
5. **Compute daje nam realną przewagę architektoniczną.** Froxelizer Filamenta działa na CPU (system jobów, limit 255 świateł i UBO 16 KB), a cały jego postprocess to fragmentowe passy; pathon przypisuje światła, liczy GTAO, wolumetrykę i sondy w compute, także w przeglądarce.
6. **Licencja:** kod Filamenta to Apache-2.0, w plikach „Copyright (C) <rok> The Android Open Source Project” (nie „Google LLC”), bez pliku NOTICE. Pochodny plik zostaje Apache-2.0, więc propozycja ADR-0047 to granica licencji na poziomie crate'u (rdzeń dalej `MIT OR Apache-2.0`).
7. **Pozwolenie autora nie obejmuje fragmentów obcych wewnątrz plików Filamenta:** SSR (McGuire i Mara, BSD-2), FXAA (NVIDIA i G3D), AgX i GT7 w `ToneMapper.cpp`, OKLab, FSR 1 (AMD, MIT), SGSR (Qualcomm, BSD-3), a GTAO powołuje się na XeGTAO. Dla SSR i FXAA bierzemy algorytm od autorów lub piszemy własny.
8. **Protokół porównania z Cycles musi porównywać z `.glb` odtworzonym w Blenderze, nie tylko z oryginalnym `.blend`.** Eksport traci materiały (37 bump, 21 korekcji koloru i inne, [scenes.md](scenes.md)), a światła obszarowe stają się spotami; tylko render `.glb` w Cyclesie mierzy sam renderer.
9. **Sufit wgpu 30** (za sekcją „Profile możliwości” planu, bez nowych pomiarów): compute, storage bufory i indirect działają na każdym profilu, w tym w przeglądarce; ray query tylko na Vulkanie; 64-bit atomics w profilu High (nie na web); multi-draw-indirect z `…Count` tylko DX12 i Vulkan (M4-11); `SUBGROUP` nie występuje na web w wgpu 30.0.2. Stąd GI przez sondy i SSGI jest realne na każdym profilu, a DDGI/ReSTIR GI z promieni kończy się na Vulkanie (M8-03).
10. **Proponowana kolejność:** protokół porównania, potem cienie lokalne i GTAO z okluzją spekularną, światła powierzchniowe (LTC), TAA, sondy odbić i SSR, szkło, sondy irradiancji i SSGI, wolumetryka, na końcu „look” (AgX, grading, dithering) i DoF. Sekcja „Proponowana kolejność prac”.

## Legenda

| Pole | Znaczenie |
| --- | --- |
| **Port** | rozmiar pracy przeniesienia na WGSL/Rust z testem i dokumentacją: **S** do dnia (jedna funkcja lub moduł, bez nowych zasobów), **M** 2–5 dni (pass lub kilka, zasoby, golden image), **L** ponad tydzień (system z CPU i GPU, strojenie, wiele passów). „Reimpl.” znaczy, że zalecam napisać od zera z publikacji (ADR-0047, pkt 3) |
| **Wpływ** | 1–5: wpływ na fotorealizm sceny wnętrza w rodzaju classroom ([scenes.md](scenes.md)); 5 = bez tego obraz wyraźnie odstaje od Cyclesa |
| **Koszt GPU** | względny koszt klatki w 1440p: niski, średni, wysoki; **szacunek z budowy algorytmu, bez pomiaru** (pomiary po M4-17) |
| **Status** | **jest** (z odnośnikiem do zadania lub crate'u), **w planie** M?-??, **brak** (nie ma ani w kodzie, ani w planie); grep po `docs/plan-implementacji.md` i `crates/` z 25.11.2026 |
| **Zależy od** | co musi istnieć wcześniej (slot, paczka, zasób) |

## Architektura Filamenta: co jest inne niż w pathon

- **System materiałów:** pliki `.mat` (YAML + GLSL) kompilowane offline do wariantów (`libs/filamat`, `tools/matc`), jedna pętla oświetlenia w `shaders/src/surface_*.fs` składana z makr (`MATERIAL_HAS_*`). pathon składa shader z modułów WESL i hooków (ADR-0021, ADR-0030), więc port to **przepisanie funkcji do modułu WESL**, nie przeniesienie systemu.
- **Passy:** `filament/src/fg/` to frame graph (odpowiednik `pathon-graph`), a efekty żyją w jednym `PostProcessManager.cpp` (4507 linii). Każdy efekt to fragmentowy full-screen quad; **compute prawie nie występuje** (jest `surface_main.cs`, ale cały postprocess to raster).
- **Światła:** do 255 świateł punktowych i spotów (`CONFIG_MAX_LIGHT_COUNT`, `libs/filabridge/include/private/filament/EngineEnums.h:113`) przypisanych do froxeli **na CPU** (`filament/src/Froxelizer.cpp`, joby), jedno światło słoneczne z cieniami i do 4 dodatkowych kierunkowych (`:160`), do 4 kaskad (`:150`).
- **Oświetlenie pośrednie:** jedno globalne IBL (prefiltrowana kostka + SH) na widok. Brak GI, sond lokalnych, lightmap.
- **Geometria:** per-renderable UBO, kości w vertex shaderze (maks. 256 kości, 256 celów morph), culling na CPU (`filament/src/Culler.cpp`). Brak GPU-driven.
- **Co jest gotowe do porównań:** `tools/diffimg` (porównanie obrazów) i `tools/cmgen` (prefiltr IBL offline), w repozytorium Filamenta na jego licencji.

## Inwentarz technik

### 1. Model materiału i BRDF

| Technika | Pliki Filamenta | Port | Wpływ | Koszt GPU | Status w pathon | Zależy od |
| --- | --- | --- | --- | --- | --- | --- |
| **GGX D, Smith height-correlated V, Schlick F** (Cook-Torrance) | `shaders/src/surface_brdf.fs:54` (`D_GGX`), `:102` (`V_SmithGGXCorrelated`, `:115` wariant fast), `:145` (`F_Schlick`); `surface_shading_model_standard.fs`; doc: `docs_src/src_markdeep/Filament.md.html` §Specular BRDF | Reimpl. S | 5 | niski | **jest** (M4-03: `crates/pathon-material/wgsl/standard.wesl`: `distribution_ggx`, `visibility_smith_ggx`, `fresnel_schlick`) | — |
| **Diffuse: Lambert, Burley, wrap** | `surface_brdf.fs:369` (`Fd_Lambert`), `:373` (`Fd_Burley`), `:382` (`Fd_Wrap`) | Reimpl. S | 2 | niski | **jest** (flaga wariantu `burley`, domyślnie Lambert z `1 − F`) | — |
| **Kompensacja wielokrotnego rozpraszania** (Kulla i Conty, Lagarde) | `surface_shading_lit.fs:204` (`getEnergyCompensationPixelParams`: `1 + f0 · (1/dfg.y − 1)`); doc §Improving the BRDFs | Reimpl. S | 3 (chropawe metale, jasne dielektryki) | niski | **częściowo**: w IBL jest (M4-06, `ibl_evaluate`); w pętli świateł punktowych nie potwierdzone | LUT DFG (jest) |
| **Clear coat** (druga warstwa GGX, V Kelemena, F0 0,04, tłumienie bazy, korekta F0 bazy) | `surface_shading_model_standard.fs:11` (`clearCoatLobe`), `surface_shading_lit.fs:170` (`getClearCoatPixelParams`), `surface_light_indirect.fs:356` (`isEvaluateClearCoatIBL`); doc §Clear coat | S (światła punktowe) / M (z IBL i SSR) | 3 (lakierowane podłogi i ławki) | niski | **częściowo**: światła punktowe jest (M4-03, ADR-0030); w IBL brak (zostawione dla `pathon-pbr`, [crates/pathon-ibl.md](crates/pathon-ibl.md)) | M4-05 |
| **Anizotropia** (GGX anizotropowy, wygięty wektor odbicia) | `surface_brdf.fs:81` (`D_GGX_Anisotropic`), `surface_lighting.fs:142`, `:157` | S | 1 | niski | **brak** (ADR-0030: poza zakresem) | tangenty w `pathon-mesh` |
| **Sheen / cloth** (Charlie D, Neubelt V, kanał DFG dla sheen, `sheenScaling`) | `surface_brdf.fs:94`, `:139`; `surface_shading_model_cloth.fs`; `surface_light_indirect.fs:390` (`evaluateClothIndirectDiffuseBRDF`); doc §Cloth | M | 2 (fotele, zasłony) | niski | **brak** (ADR-0030: poza zakresem) | trzeci kanał LUT DFG (dziś `brdf_lut` w `pathon-ibl`) |
| **Subsurface tani** (przybliżone BTDF z grubością, kolorowa penumbra) i **transmisja dyfuzyjna** | `surface_shading_model_subsurface.fs` (46 linii), `surface_shading_lit.fs:127` (`getDiffuseTransmissionPixelParams`) | S | 2 (papier, zasłony, liście) | niski | **częściowo**: `diffuse_transmission` jest (M4-03); prawdziwy SSS to M5-04 (Filament go nie ma) | M4-05 |
| **Iridescencja** (cienka warstwa, Belcour i Barla, KHR_materials_iridescence) | `surface_brdf.fs:160–290` | M | 1 | niski | **brak** | — |
| **Drugi lob spekularny, retroreflection, glossiness** | `surface_shading_lit.fs`, `surface_light_indirect.fs` (`MATERIAL_HAS_SECOND_SPECULAR_LOBE`, `RETROREFLECTION`) | S każdy | 1 | niski | **brak** | — |
| **Geometryczne specular AA** (Kaplanyan 2016, Tokuyoshi i Kaplanyan 2019) | `surface_lighting.fs:102` (`computeKernelRoughness`), `:112` (`normalFiltering`) | Reimpl. S | 4 (migotanie odblasków na krzywiznach) | niski | **brak**; kandydat do M4-05 | pochodne ekranowe (`dpdx`/`dpdy`): nie działa w ścieżce compute / visibility buffer (ADR-0030: „bez pochodnych”) |
| **Micro-shadowing** (Chan 2018) | `surface_lighting.fs:94` | Reimpl. S | 2 | niski | **brak** | AO materiału lub slotu `ao` |

### 2. Światła i jednostki

| Technika | Pliki Filamenta | Port | Wpływ | Koszt GPU | Status w pathon | Zależy od |
| --- | --- | --- | --- | --- | --- | --- |
| **Jednostki fotometryczne i pre-ekspozycja** (lux, lumen, kandela; EV100 z apertury, migawki, ISO) | `shaders/src/common_graphics.fs:19` (`computePreExposedIntensity`), `filament/src/Exposure.cpp`; doc §Units | S | 5 (spójna jasność) | niski | **jest** (kontrakt slotów: lux i lumen, slot `exposure`; M2-10, M4-09) | — |
| **Directional i SUN** (dysk słońca jako światło powierzchniowe przez „reprezentatywny punkt”; do 4 dodatkowych kierunkowych) | `shaders/src/surface_light_directional.fs:9` (`sampleSunAreaLight`) | S | 3 (połysk słońca na gładkich materiałach) | niski | **częściowo**: światło kierunkowe jest w `pathon-scene` (M4-02); dysk słońca brak | M4-05 |
| **Punctual: point, spot** (okno falloffu `getSquareFalloffAttenuation`, stożek przez scale i offset, FOCUSED_SPOT vs SPOT) | `shaders/src/surface_light_punctual.fs:93`, `:101`, `:111`, `:126`; `filament/src/components/LightManager.cpp` | S | 5 | niski | **w planie** M4-05 (clustered forward+) | froxele (sekcja 6) |
| **Światła powierzchniowe** | Filament: brak (docs §Area lights „[TODO]”) | — | 4 | średni | **brak**; patrz „Gdzie możemy być lepsi” G3 | M4-05 |
| **Profile IES** | Filament: brak (grep po `.ies`, `IES profile` w `filament/`, `libs/`, `tools/` bez trafień; `colorIES` w shaderze to nazwa pola) | — | 2 | niski | **brak** | M4-05 |
| **Kanały świateł** (maska obiekt–światło, 8 kanałów) | `surface_light_punctual.fs` (`light.channels`) | S | 1 | niski | **brak** | M4-05 |

### 3. IBL i okluzja pośrednia

| Technika | Pliki Filamenta | Port | Wpływ | Koszt GPU | Status w pathon | Zależy od |
| --- | --- | --- | --- | --- | --- | --- |
| **Prefiltrowana radiancja** (kostka z mipami = chropowatość, dominujący kierunek odbicia) | `surface_light_indirect.fs:115`, `:122`, `:131`; `libs/ibl/src/CubemapIBL.cpp`, `libs/iblprefilter/src/IBLPrefilterContext.cpp` (GPU), `tools/cmgen` | M | 5 | niski (runtime), jednorazowy prefiltr | **jest** (M4-06, [ADR-0037](adr/0037-ibl-srodowisko-prefiltr-i-lut.md), compute) | — |
| **Irradiancja diffuse: SH (L2, 9 współczynników)** | `surface_light_indirect.fs:43`, `libs/ibl/src/CubemapSH.cpp` | S | 3 | niski | **częściowo**: zamiast SH jest kostka irradiancji 32² (ADR-0037); SH9 otwarte dla słabego mobile'a | — |
| **LUT DFG (split-sum, multiscatter, kanał sheen)** | `filament/src/DFG.cpp` (ładuje wygenerowany blob), `libs/ibl/src/CubemapIBL.cpp:1007` (`DFG`), `surface_light_indirect.fs:22` | Reimpl. S | 4 | niski | **jest** (M4-06, LUT 128² w compute; bez kanału sheen) | — |
| **IBL przez importance sampling** (bez prefiltra; wariant jakościowy) | `surface_light_indirect.fs:174–386` | M | 1 | wysoki | **brak** (niepotrzebne) | — |
| **Specular occlusion** (Lagarde z AO; stożki z bent normals Jimeneza) i **multi-bounce AO** | `shaders/src/surface_ambient_occlusion.fs:74` (`SpecularAO_Lagarde`), `:105` (`SpecularAO_Cones`), `:181` (`gtaoMultiBounce`) | Reimpl. S | 5 (bez tego IBL „świeci” w zakamarkach wnętrza) | niski | **brak** (ADR-0037: zostawione dla `pathon-pbr`); M5-02 nie wspomina bent normals | GTAO (sekcja 7) |
| **Horizon occlusion** (przyciemnienie odbić pod horyzontem normal mapy) | Filament: brak (grep `horizon` w `shaders/` trafia tylko w GTAO) | Reimpl. S | 2 | niski | **brak** | — |
| **Skybox, rotacja i intensywność IBL** | `filament/src/Skybox.cpp`, `filament/src/materials/skybox.mat`, `filament/src/details/IndirectLight.cpp` | S | 2 | niski | **w planie** M4-08 (`pathon-sky`) | — |

### 4. Odbicia, szkło, przezroczystość

| Technika | Pliki Filamenta | Port | Wpływ | Koszt GPU | Status w pathon | Zależy od |
| --- | --- | --- | --- | --- | --- | --- |
| **SSR** (DDA po `structure` w stylu McGuire i Mara; kolor z historii poprzedniej klatki z mipami per chropowatość; tylko opaque, roughness < √0,5, tylko materiały z trybem screen-space; osobny pass geometrii z wariantem SSR) | `shaders/src/surface_light_reflections.fs`, `filament/src/PostProcessManager.cpp:928` (`ssr`), `:1647` (`prepareMipmapSSR`), `:1811` (`generateMipmapSSR`), `filament/src/ds/SsrPassDescriptorSet.*` | L (Reimpl., hi-Z) | 4 (podłoga, tablica, szyby) | średni–wysoki | **w planie** M5-06 (hi-Z trace na `depth_pyramid`; lepszy niż liniowy DDA Filamenta). **Uwaga licencyjna:** kod śladu to BSD-2 McGuire i Mara, poza zezwoleniem na Filament | `depth_pyramid`, `roughness`, kolor poprzedniej klatki (historia grafu), TAA |
| **Refrakcja i transmisja** (kula pełna, pudełko, cienka powłoka; kolor z zmipmapowanego bufora ekranu albo kostki; absorpcja Beera) | `surface_light_indirect.fs:470–736` (`refractionSolidSphere:505`, `refractionSolidBox:512`, `refractionThinSphere:527`, `evaluateRefraction:544`), `PostProcessManager.cpp:1811` (kopia koloru z mipami) | M | 3 (szyby okien) | średni (kopia i mipmapy koloru) | **w planie** M4-15; flagi `transmission` i `volume` są w M4-03 | M4-05, kopia koloru opaque |
| **Dyspersja spektralna (N=4)** | `surface_light_indirect.fs:590` (`calculateDispersion`), `tools/specgen` | M | 1 | średni | **brak** | refrakcja |
| **OIT / sortowanie przezroczystych** | Filament: sortowanie od tyłu, bez OIT | S | 2 | niski | **w planie** M4-15 (OIT w „otwartych decyzjach”) | M4-05 |

### 5. Cienie

| Technika | Pliki Filamenta | Port | Wpływ | Koszt GPU | Status w pathon | Zależy od |
| --- | --- | --- | --- | --- | --- | --- |
| **Cascaded shadow maps** (do 4 kaskad, stabilizacja, LiSPSM, focus na casterach i receiverach, atlas) | `filament/src/ShadowMap.cpp:173` (LiSPSM), `:189` (focus), `filament/src/ShadowMapManager.cpp`, `shaders/src/surface_shadowing.fs` | L | 5 | średni | **w planie** M4-07 (CSM + PCF) | M4-05 (depth, kamera) |
| **Cienie spotów i świateł punktowych** (atlas, 6 ścian dla point, `getPointLightFace`) | `ShadowMapManager.cpp`, `surface_shadowing.fs:485` | M | 5 (wnętrza oświetlone lampami sufitowymi) | średni–wysoki | **brak w planie** (M4-07 mówi tylko o CSM + PCF); kandydat do rozszerzenia M4-07 | M4-07 |
| **PCF** (sprzętowy hard; low: filtr 3×3 Castaño) | `surface_shadowing.fs:20–100` | S | 3 | niski | **w planie** M4-07 | — |
| **VSM / EVSM** (momenty wykładnicze, rozmycie Gaussa, mipmapy, redukcja light-bleed) | `surface_shadowing.fs:101–210` (`evaluateEVSM:162`, `ShadowSample_VSM:187`), `filament/src/materials/evsm/*`, `ShadowMap.cpp` (`computeVsmLightSpaceMatrix`) | M | 3 | średni (rozmycie, tekstury RGBA16F/32F) | **brak** | filtrowalna tekstura float (na web `float32-filterable` jest opcjonalne) |
| **PCSS na EVSM** (przybliżone O(1) wyszukiwanie blokera z momentów, penumbra z promienia żarówki, cap `maxPenumbraRatio`; `DPCF` to przestarzały alias) | `surface_shadowing.fs:211–390` (krok 1 w `:244`), `include/filament/Options.h` (`SoftShadowOptions`), `LightManager.h` (`bulbRadius`) | M | 4 (miękkie cienie, przybliżenie świateł powierzchniowych) | średni | **brak** | EVSM |
| **Contact shadows w przestrzeni ekranu** | `surface_shadowing.fs:391–464` (`screenSpaceContactShadow:425`), `PostProcessManager.cpp:606` (`structure`) | S | 3 | niski | **brak** | głębia / `depth_pyramid` |
| **SSCT** (cone tracing dominującego światła w passie AO, Naughty Dog) | `filament/src/materials/ssao/ssct.fs`, `ssctImpl.fs` | M | 2 | średni | **brak** | AO |

### 6. Oświetlenie klastrowe

| Technika | Pliki Filamenta | Port | Wpływ | Koszt GPU | Status w pathon | Zależy od |
| --- | --- | --- | --- | --- | --- | --- |
| **Froxele** (siatka frustum, plasterki wykładnicze po z, CPU w jobach, bufor rekordów w UBO; limit 255 świateł; sfery ograniczające dla spotów) | `filament/src/Froxelizer.{h,cpp}` (`computeFroxelLayout:286`, joby `:728`), `shaders/src/surface_light_punctual.fs:23` (`getFroxelCoords`), doc §Clustered Forward Rendering i Annex §Light assignment with froxels | M (przepisać na compute: reimpl. z doc) | 5 (classroom ma kilkanaście świateł) | niski–średni | **w planie** M4-05 (przypisanie w compute, także na web) | M4-02 (światła w scenie) |

### 7. Okluzja otoczenia i mgła

| Technika | Pliki Filamenta | Port | Wpływ | Koszt GPU | Status w pathon | Zależy od |
| --- | --- | --- | --- | --- | --- | --- |
| **SAO** (McGuire, mipmapy głębi, rozmycie dwustronne) | `filament/src/materials/ssao/saoImpl.fs`, `sao.mat`, `mipmapDepth.mat`, `bilateralBlur.mat`, `PostProcessManager.cpp:1029`, `:1321` | M | 4 | średni | **brak**; M5-02 wybiera GTAO, więc SAO niepotrzebne | `depth_pyramid` |
| **GTAO** (sloty kierunków, wariant z bitmaskami widoczności, heurystyka grubości, `linearThickness`) | `ssao/gtaoImpl.fs` (223 linie), `gtao.mat`; nagłówek powołuje się na XeGTAO (Intel) i repozytorium Unity | M (compute; port albo reimpl. z XeGTAO i artykułu) | 5 | średni | **w planie** M5-02 (compute, wszystkie profile) | `depth`, `normals`, `depth_pyramid` (M4-05) |
| **Bent normals z passu AO** (do diffuse IBL i stożków specular occlusion) | `ssao/gtaoBentNormals.mat`, `saoBentNormals.mat`, `bilateralBlurBentNormals.mat`, `surface_ambient_occlusion.fs:126` | M (część GTAO) | 4 | niski (dodatkowy wynik) | **brak**; `Slot::AO` to `R8Unorm`, więc potrzebny slot deklarowany przez paczkę (`Slot::new`, bez zmiany `pathon-api`) | GTAO |
| **Mgła wykładnicza z wysokością** (Wenzel; in-scattering słońca, kolor z IBL, dystans odcięcia) | `shaders/src/surface_fog.fs:8`, `filament/src/materials/fog/fog.mat`, `PostProcessManager.cpp:2885`, `Options.h` (`FogOptions`) | S | 2 | niski | **brak** | `hdr`, `depth` |

### 8. Antyaliasing, upscaling, rozdzielczość

| Technika | Pliki Filamenta | Port | Wpływ | Koszt GPU | Status w pathon | Zależy od |
| --- | --- | --- | --- | --- | --- | --- |
| **TAA** (jitter Halton 2-3 x8/x16/x32, RGSS, helix; historia Catmull-Rom; odjittering Lanczos-2; klip YCoCg i wariancji; dezokluzja po głębi; adaptacja do ruchu; blend w przestrzeni tonemapowanej; pole `lodBias`) | `filament/src/materials/antiAliasing/taa/taa.mat` (795), `taa.md` (311, dokument projektowy), `PostProcessManager.cpp:3095` (`TaaJitterCamera`), `:3189`, `:3257` (`taa`), `:205` (sekwencje) | L (port z `taa.md`, strojenie) | 5 | niski–średni | **w planie** M5-01 (jitter, historia, clamping YCoCg) | `motion` i `depth` (M4-05), `prepare_view` z jitterem (jest), historia grafu (M2-08) |
| **MSAA i custom resolve** (resolve po tonemappingu) | `Options.h:539`, `materials/colorGrading/customResolveAsSubpass.mat`, `PostProcessManager.cpp:4110` | M | 2 | wysoki (pamięć) | **w planie** M4-19 (tylko decyzja) | — |
| **FXAA** | `materials/antiAliasing/fxaa/fxaa.fs` (NVIDIA FXAA 3.11 + G3D) | S | 1 | niski | **w planie** M5-05; **licencja obca**, osobna ocena | — |
| **FSR 1** (EASU, RCAS) i **SGSR 1**, upscale bilinear | `PostProcessManager.cpp:3518` (`rcas`), `:3598`, `:3679` (SGSR), `:3751` (`upscaleFSR1`), `filament/src/materials/fsr/*` (AMD, MIT), `materials/sgsr/*` (Qualcomm, BSD-3) | S–M | 3 (wydajność) | niski | **w planie** M5-03 (własny WGSL, MIT) | — |
| **Dynamiczna rozdzielczość** (regulator PID po czasie klatki, skala per oś, homogeniczna) | `Options.h:80` (`DynamicResolutionOptions`), `filament/src/PIDController.h`, `FrameSkipper.*`, `FrameInfo.cpp`, `details/Renderer.cpp` | M | 3 (stabilność klatek) | oszczędza | **częściowo**: `render_size` jest w `ViewCtx` (M2-10); regulatora brak | timestampy passów (M2-12), TAA lub FSR |

### 9. Postprocess i kolor

| Technika | Pliki Filamenta | Port | Wpływ | Koszt GPU | Status w pathon | Zależy od |
| --- | --- | --- | --- | --- | --- | --- |
| **Bloom** (downsample 13-tap z Karis, upsample, próg, dirt, poziomy jakości) | `materials/bloom/*`, `PostProcessManager.cpp:2449` | M | 3 (okna) | niski | **jest** (M4-09, `pathon-post`) | — |
| **Lens flare** (ghosty, halo, starburst, aberracja chromatyczna) | `materials/flare/flare.mat`, `PostProcessManager.cpp:2674` | M | 1 | niski | **brak** | bloom |
| **DoF** (gather „Life of a Bokeh”, kafelki, dilate, median, kombinacja) | `materials/dof/*` (9 materiałów, `dof.mat` 666 linii), `PostProcessManager.cpp:1864` | L | 2 | wysoki | **brak** | `depth`, bloom |
| **Winieta** | `shaders/src/inline_vignette.fs` (22 linie) | S | 1 | niski | **brak** | — |
| **Color grading** (LUT 3D: balans bieli, mikser kanałów, SMH, CDL, krzywe, kontrast, vibrance, saturacja, gamut mapping; adaptacja nocna) | `filament/src/details/ColorGrading.cpp`, `materials/colorGrading/colorGrading.{mat,fs}`, `PostProcessManager.cpp:2900`, `ColorSpaceUtils.cpp` (fragment OKLab: MIT, obcy) | M | 3 | niski | **brak** | M4-09 |
| **Tone mappery:** ACES (fitted i legacy), Filmic, PBR Neutral, GT7, AgX (z looks), Generic | `filament/src/ToneMapper.cpp` (`:141` ACES, `:447` PBR Neutral, `:503` GT7 — MIT Polyphony, `:690` AgX — źródła obce) | S każdy | 4 (charakter obrazu) | niski | **częściowo**: ACES fitted i Khronos PBR Neutral są (M4-09); AgX, GT7: brak, osobna ocena licencji | M4-09 |
| **Dithering** (interleaved gradient noise, Vlachos, szum trójkątny, temporalny) | `shaders/src/inline_dithering.fs` | S | 2 (banding na gładkich ścianach) | niski | **brak** (otwarte w M4-09) | — |
| **Ekspozycja fizyczna i automatyczna** | `filament/src/Exposure.cpp`; auto-ekspozycja: tylko w docs (§Automatic exposure), **w kodzie brak** | S | 4 | niski | **jest** (M4-09: ręczna i histogram, adaptacja w log2) | — |

### 10. Geometria, ruch, infrastruktura

| Technika | Pliki Filamenta | Port | Wpływ | Koszt GPU | Status w pathon | Zależy od |
| --- | --- | --- | --- | --- | --- | --- |
| **Wektory ruchu** (pass tylko dla poruszonych obiektów, RG16F; ruch kamery z reprojekcji głębi w TAA) | `PostProcessManager.cpp:796` (`motionVectors`), `shaders/src/surface_instancing.glsl` (`prevWorldFromWorldMatrix`), `surface_main.vs` (`VARIANT_HAS_MOT`) | M | 5 dla TAA | niski | **w planie** M4-05 (prepass `motion`), dane poprzedniej klatki są (M4-01, `TransformHistory`) | M4-01 |
| **Pass struktury** (zmipmapowana głębia dla AO i contact shadows) | `PostProcessManager.cpp:606` (`structure`) | S | 3 | niski | **w planie** M4-05 (`depth_pyramid`, Hi-Z) | — |
| **Instancing** | `shaders/src/surface_instancing.glsl`, `filament/src/details/InstanceBuffer.cpp` | — | 1 | niski | **inny projekt**: storage buffer i indirect (M4-11); nic do portowania | M4-11 |
| **Skinning i morph targets** (w vertex shaderze, UBO) | `filament/src/details/SkinningBuffer.cpp`, `MorphTargetBuffer.cpp`, `shaders/src/surface_main.vs:63` | M (Reimpl.) | 2 | niski | **w planie** M4-12 (skinning w compute, pozycje poprzedniej klatki) | M4-01 |
| **Picking** (ID obiektu w passie, odczyt pod kursorem) | `filament/src/details/View.cpp`, `PostProcessManager.cpp:736` (`transparentPicking`), `shaders/src/surface_depth_main.fs` | S | 0 | niski | **w planie** M5-09 | `material_id` |
| **Frame graph, deskryptory, warianty materiałów** | `filament/src/fg/*`, `filament/src/ds/*` | — | — | — | **inny projekt** (`pathon-graph`, `pathon-rhi`); nie portujemy | — |

## Gdzie możemy być lepsi niż Filament

Ocena uczciwa: „lepiej” oznacza tu *dodatkowe* zdolności lub zdolności tańsze dzięki compute, a nie wyższą jakość w tych samych technikach. Każda pozycja ma przy ryzyku odniesienie do tego, co wgpu 30 i WebGPU pozwalają (compute i storage bufory na każdym profilu, w tym w przeglądarce; ray query tylko Vulkan; 64-bit atomics w profilu High, nie na web; multi-draw-indirect z `…Count` tylko DX12 i Vulkan; w wgpu 30.0.2 `SUBGROUP` nie występuje na web; zapis do tekstur storage zależy od formatu, do sprawdzenia przy każdej technice).

**Priorytet:** P1 = następne w kolejce, P2 = po P1, P3 = później lub warunkowo, „poza sufitem” = nie zrobimy na wszystkich profilach.

| # | Pomysł | Filament | Po co we wnętrzu | Praca / GPU | Ryzyko i sufit wgpu 30 | Prior. |
| --- | --- | --- | --- | --- | --- | --- |
| G1a | **Sondy irradiancji** (siatka sond w compute: oktaedralne mapy irradiancji + głębi do wyciekania, aktualizacja z kostek rasteryzowanych albo z śladów w przestrzeni ekranu; „DDGI-lite bez RT”) | brak GI | światło odbite od ścian, sufitu i podłogi: największa różnica między forward PBR a Cyclesem we wnętrzu | L / średni (sondy aktualizowane stopniowo) | **Średnie:** wyciekanie światła przez ściany (potrzeba wizyjności jak w DDGI), pamięć i koszt aktualizacji; wszystkie profile OK (compute + storage). Bez promieni jakość pochodzi z rasteryzacji sceny do kostek | P1 (po GTAO i TAA) |
| G1b | **SSGI** (half-res, hi-Z trace, akumulacja temporalna i denoise) | brak | dopełnienie sond; kolor odbity od widocznych powierzchni | L / średni–wysoki | **Wysokie:** brak informacji spoza ekranu, szum i ghosting, trudne do strojenia; potrzebny wspólny denoiser (G14); wszystkie profile OK | P2 |
| G1c | **Sondy / lightmapy wypiekane w Cyclesie** (offline, import jako wolumen irradiancji) | brak | tani parytet dla statycznego classroom i pomiar sufitu | M / niski | Niskie technicznie, ale to nie dynamiczne GI; służy jako eksperyment („ile brakuje do Cyclesa przy idealnym GI”) i dla scen statycznych | P2 (jako narzędzie pomiarowe) |
| G1d | **DDGI i ReSTIR GI z promieni** (ray query) | brak | pełne GI z cieniami i wieloma odbiciami | XL | **Poza sufitem:** wgpu 30 ma ray query tylko na Vulkanie (M8-03); bez DX12, Metalu i web | P3, tylko Vulkan |
| G1e | **VXGI, SDF GI** | brak | — | XL | **Odradzam:** pamięć, spójność, duże ryzyko, słaba przenośność na web | nie |
| G2 | **Lokalne sondy odbić** (kostki z korekcją paralaksy, mieszanie sąsiednich, prefiltr przez builder z M4-06) | jedno globalne IBL | odbicia na podłodze i szybach wnętrza pokazują wnętrze, nie niebo z zewnątrz | M–L / niski–średni | Niskie: znana technika; builder prefiltra już jest (`IblBuilder`); koszt pamięci i rasteryzacji sześciu ścian; wszystkie profile OK | P1 |
| G3 | **Światła powierzchniowe LTC** (rect, dysk; Heitz 2016) | brak (docs „[TODO]”) | classroom ma światła prostokątne (wypełnienie za oknem, podświetlenie tablicy), Cycles daje szerokie odblaski i miękkie oświetlenie | M / średni (kilkanaście świateł OK) | **Licencja i dane:** tablice LTC (dopasowane macierze) to dane z osobnego źródła, trzeba sprawdzić licencję albo policzyć własny fit (praca). Cienie dla świateł powierzchniowych wymagają PCSS (G4). Wszystkie profile OK. `LightKind` jest `#[non_exhaustive]`, więc dodanie wariantu nie łamie API | P1 |
| G4 | **Miękkie cienie PCSS z akumulacją temporalną** (klasyczny blocker search po mapie głębi, stochastyczny, wygładzany TAA) | PCSS na EVSM (szybki, z przeciekaniem światła i teksturą RGBA float) | penumbra rośnie z odległością od okludera, jak od światła powierzchniowego | M / średni | Niskie. Uczciwie: port PCSS na EVSM Filamenta jest alternatywą tańszą i z gorszą jakością (light bleeding, wymaga filtrowanej tekstury float, na web opcjonalne); nasz wariant jest cięższy, ale czystszy. Wymaga TAA | P1–P2 |
| G5 | **GTAO w compute z bent normals i okluzją spekularną** | **ma** (GTAO, bent normals, multi-bounce, stożki) | parytet, nie przewaga | M / średni | Niskie. Różnica kosztu: nasz pass compute w połowie rozdzielczości z upsamplingiem świadomym głębi; bent normals jako slot paczki | P1 (parytet) |
| G6 | **TAA ponad Filament:** maska reaktywna (przezroczyste, cząsteczki; slot `reactive` już jest), dylatacja wektorów ruchu (najbliższa głębia 3×3), odrzucanie historii dla odbić (ruch odbitego obrazu, `roughness`), stabilność przy specular | TAA bez maski reaktywnej i bez dylatacji prędkości (grep `reactive`, `dilat` w `taa.mat`) | mniej smug na szkle, mniej duchów na odbiciach podłogi | M (nad parytetem) / niski | Niskie. Najpierw parytet z M5-01, potem rozszerzenia | P2 |
| G7 | **Specular AA także dla normal map** (Toksvig lub wariancja normalnych wypiekana do mipów chropowatości przy generowaniu mipmap, `pathon-texture` już robi mipy normal map w compute) | tylko geometryczne (pochodne normalnej geometrycznej) | mniej migotania na teksturowanych powierzchniach (tablice korkowe, drewno) | S–M / niski | Niskie; wszystkie profile OK | P2 |
| G8 | **Wolumetryka: mgła froxelowa i smugi światła** (siatka ok. 160×90×64 w compute, próbkowanie map cieni, akumulacja temporalna) | tylko analityczna mgła wysokościowa | oryginalny `.blend` ma stożki objętościowe i kurz w oknach (`_volumeLight`, `_dustParticules`); smugi słońca w oknach dają „powietrze” | M–L / średni | Średnie: zależy od cieni lokalnych i TAA; tekstury 3D `rgba16float` do zapisu w compute działają na web; strojenie ilości próbek | P2 |
| G9 | **Ekspozycja lepsza niż Filament:** auto-ekspozycja jest (Filament nie ma jej w kodzie); dodać metering ważony środkiem i priorytet świateł, kompensację, lokalną ekspozycję dla jasnych okien | brak auto-ekspozycji | wnętrze z oknami ma duży rozrzut luminancji; globalna ekspozycja albo spala okna, albo ciemni wnętrze | S–M / niski | Niskie. Lokalny tone mapping (np. siatka dwustronna) to L z ryzykiem artefaktów halo | P2 |
| G10 | **GPU-driven rendering:** culling frustum i Hi-Z w compute, indirect draw, bindless materiały | culling CPU w jobach, draw per renderable | wydajność dużych scen, nie wygląd | M–L (M4-11, M4-04 jest) / zysk | **Web:** M4-11 przewiduje warianty (z `…Count` na DX12 i Vulkanie, bez na Metalu); stan na web do sprawdzenia w tym zadaniu. Dla 443 tys. trójkątów classroom CPU wystarcza, więc to nie blokuje wyglądu | P2 (równolegle) |
| G11 | **Meshlety i LOD** | brak | wydajność milionów trójkątów | XL | **Ryzyko wysokie:** rasteryzacja w compute na 64-bit atomics (tylko Desktop), bez mesh shaderów; classroom tego nie potrzebuje | P3 (M8-02) |
| G12 | **ReSTIR-lite** (resampling świateł) | brak | wiele świateł emisyjnych | XL | **Niski zwrot:** w classroom jest kilkanaście świateł, a clustered forward+ je obsłuży; ReSTIR GI wymaga promieni (Vulkan) | nie teraz |
| G13 | **Światło dzienne i atmosfera** (nieboskłon proceduralny jako źródło IBL, słońce zgodne z ekspozycją) | skybox + dysk słońca, bez atmosfery | okna wnętrza oświetlone spójnym niebem | M / średni | Niskie; M4-08 w planie | P2 (M4-08) |
| G14 | **Wspólny denoiser** (à-trous i akumulacja temporalna, crate `pack-lib`, ADR-0041) dla G1b, G4, G8 i AO | brak współdzielonego denoisera | jeden kod i jedna regulacja zamiast trzech | M / niski | Niskie; musi iść przed SSGI | P1–P2 (przed G1b) |

Co **nie** jest przewagą, choć kusi: bent-normal specular occlusion, GTAO z multi-bounce, geometryczne specular AA, SSR, refrakcja z dyspersją, PCSS i froxele Filament ma w produkcji; tu celujemy w parytet, a przewagą bywa tylko implementacja w compute.

## Proponowana kolejność prac

Zasada: od największej różnicy w obrazie wnętrza na jednostkę kosztu, z uwzględnieniem zależności. To lista **kandydatów** (nazwy robocze `K01…K22`); numery M i wpisy w Linearze nada orkiestrator. Kolumna „Plan” wskazuje istniejące zadanie, które kandydat rozszerza albo zastępuje.

| # | Kandydat | Zakres | Port | Zależy od | Plan | Efekt, który mierzymy względem Cyclesa |
| --- | --- | --- | --- | --- | --- | --- |
| K01 | `reference-protocol` | Protokół porównania z Cyclesem (sekcja niżej), skrypt Blendera i komparator; **pierwszy**, bo mierzy resztę | M | classroom-export (jest), loader M4-10 po stronie pathon | nowe | baza: FLIP, SSIM, ΔE i bias dla każdej kamery |
| K02 | `pbr-core` | M4-05 z dopisanym: geometryczne specular AA, micro-shadowing, kompensacja wielokrotnego rozpraszania w świetle punktowym, clearcoat w IBL, hook okluzji spekularnej | S–M (Reimpl.) | M4-03, M4-06 | M4-05 | spadek biasu na błyszczących powierzchniach |
| K03 | `shadow-local` | M4-07 rozszerzone: spoty i światła punktowe w atlasie, stabilizacja kaskad, contact shadows | M–L | M4-05 | M4-07 | cienie od lamp sufitowych i słońca; maska „zacienione” w ΔE |
| K04 | `ao-gtao` | M5-02 z bent normals (slot paczki), multi-bounce AO i specular occlusion ze stożków | M (port albo reimpl.) | M4-05 (`depth_pyramid`) | M5-02 | pośrednie przyciemnienie zakamarków, mniej wycieku IBL |
| K05 | `area-lights-ltc` | światła powierzchniowe (rect, dysk) w `pathon-scene` i `pathon-pbr`, tablice LTC | M | M4-05, ocena licencji LTC | nowe | pasma odblasków i miękkość oświetlenia; wymaga `extras.pathon_area` z eksportu |
| K06 | `taa` | M5-01 (parytet z Filamentem), potem maska reaktywna i dylatacja prędkości jako druga faza | L | `motion`, jitter, historia | M5-01, M5-08 | brak aliasingu przy stałej kamerze, zbieżność do referencji po N klatkach |
| K07 | `denoise-lib` | wspólny denoiser (`pack-lib`): à-trous, akumulacja temporalna | M | K06 | nowe | warunek dla K10, K12, K13 |
| K08 | `reflection-probes` | lokalne sondy odbić z paralaksą, mieszanie, prefiltr przez `IblBuilder` | M–L | M4-06, M4-05 | nowe | odbicia podłogi i szyb |
| K09 | `ssr` | M5-06 (hi-Z na `depth_pyramid`, fallback do sond K08) | L (Reimpl.) | M4-05, K06, K08 | M5-06 | odbicia obiektów w podłodze i tablicy |
| K10 | `shadow-soft` | PCSS z akumulacją temporalną dla słońca i świateł powierzchniowych | M | K03, K05, K06, K07 | nowe | penumbry cieni |
| K11 | `glass` | M4-15: transmisja i refrakcja szyb, kopia koloru opaque | M | M4-05 | M4-15 | okna i szkło; maska „szkło” |
| K12 | `gi-probes` | siatka sond irradiancji (relight w compute), wizyjność, integracja z K04 | L | K04, K08, K07 | nowe | główny spadek biasu w „Indirect Diffuse” (pass Cyclesa) |
| K13 | `ssgi` | half-res SSGI jako uzupełnienie sond | L | K07, `depth_pyramid`, K06 | nowe | dodatkowa poprawa w pobliżu ekranu |
| K14 | `volumetrics` | froxelowa mgła i smugi światła, próbkowanie map cieni | M–L | K03, K06, K07 | nowe | smugi w oknach |
| K15 | `look` | dithering, AgX i GT7 (po ocenie licencji), color grading LUT, winieta, lokalna ekspozycja | S–M | M4-09 | M4-09 (dopełnienie) | ΔE po wspólnym tonemapie, zgodność odczuć |
| K16 | `upscale-dynres` | M5-03 (FSR 1) i regulator dynamicznej rozdzielczości | M | timestampy (M2-12), K06 | M5-03 | wydajność przy zachowaniu FLIP |
| K17 | `sky` | M4-08 | M | — | M4-08 | spójne niebo i IBL |
| K18 | `gpu-culling` | M4-11 (frustum i Hi-Z, indirect) | M–L | M4-05 | M4-11 | wydajność (bez wpływu na obraz) |
| K19 | `sss`, `skin`, `picking`, `fxaa` | M5-04, M4-12, M5-09, M5-05 (FXAA po ocenie licencji) | S–M każdy | jak w planie | M5-04, M4-12, M5-09, M5-05 | nie dotyczą classroom |
| K20 | `dof-flare` | DoF i lens flare | L / M | `depth`, bloom | nowe | tylko efekty fotograficzne |
| K21 | `meshlet` | M8-02 | XL | M4-05, 64-bit atomics | M8-02 | wydajność milionów trójkątów |
| K22 | `rt-gi` | M8-03 (ray query, Vulkan) | XL | backend Vulkan | M8-03 | sufit jakości na Vulkanie |

Uwagi do kolejności:

- **K01 idzie pierwszy**, bo bez pomiaru nie wiemy, co daje największą różnicę. Strona Blendera (render referencji) nie zależy od pathon i może ruszyć od razu.
- K02, K03, K04 to najtańsza droga do „wnętrza, które wygląda jak wnętrze”: światło lokalne z cieniami, ciemniejsze zakamarki i brak „świecącego” IBL w rogach. K04 przed K06, bo GTAO obejdzie się prostym blurem dwustronnym.
- K06 (TAA) przed K10, K12, K13, K14, bo te techniki opierają się na akumulacji temporalnej; K07 (denoiser) dzieli kod między nimi.
- K08 przed K09: sondy to fallback SSR dla promieni, które uciekają poza ekran.
- Kandydaci K12 i K13 to największa niewiadoma. Zalecam ocenę po K01: jeśli różnica w „Indirect Diffuse” po K02–K05 i K08 jest już mała, K12 może spaść w kolejce; eksperyment G1c (wypiekane GI z Cyclesa) pokaże sufit.
- Ścieżkę minimalną planu (★) kandydaci nie zmieniają.

## Protokół porównania z Cyclesem (scena classroom)

Cel: mierzalne porównanie renderu pathon z renderem referencyjnym Blendera, powtarzalne co zmianę techniki. Opis scen i eksportu: [scenes.md](scenes.md). **To jest specyfikacja: skryptu ani komparatora nie implementujemy w tym zadaniu.**

### Dwa wzorce, dwa pytania

| Wzorzec | Co renderuje Cycles | Na co odpowiada |
| --- | --- | --- |
| **A. `classroom.blend`** (oryginał) | oryginalne materiały, światła obszarowe, okna dzienne (portal) | ile brakuje pathon do „prawdziwego” obrazu; mieszają się straty eksportu (37 × bump, 21 korekcji koloru, 12 miksów tekstur, 16 atrybutów obiektu, itd.) i różnice renderera |
| **B. `classroom.glb` odtworzony w Blenderze** (import przez `import_scene.gltf`, światła obszarowe odtworzone z `extras.pathon_area`, portale pominięte tak jak w eksporcie) | to samo, co dostaje pathon | **mierzy sam renderer**; to metryka do bramkowania zmian |

Wzorzec B jest metryką główną, A jest pomocniczy (pokazuje łączną lukę i sufit eksportu).

### Ustawienia wspólne (do zapisania w `reference.json` obok wyników)

- **Kamery:** `renderCam`, `pathon_wide`, `pathon_reverse` z `<nazwa>.scene.json` (pozycja, `yfov`, `znear`, `zfar`); rozdzielczość 1920×1080, proporcje 16:9. Blender: ten sam `yfov` (przez ogniskową z sensora 32 mm), `sensor_fit` zgodny.
- **Warianty oświetlenia:** `lights-only` (świat czarny jak w oryginale, `sun` + światła punktowe, spoty i obszarowe + emisja) oraz `ibl` (świat w Blenderze i IBL w pathon z tego samego `env/Garage.hdr`, ta sama siła i obrót). Wariant `ibl` testuje IBL, AO i okluzję spekularną; `lights-only` testuje światła, cienie i GI.
- **Ekspozycja i kolor:** Blender `Standard`, look `None`, ekspozycja 0, gamma 1 ([scenes.md](scenes.md)). Wyniki w liniowym Rec.709 (EXR, float 32). Jedna stała kalibracyjna ekspozycji na scenę i wariant, **wyznaczona raz** na plamie diffuse oświetlonej wyłącznie słońcem (porównanie pass `Diffuse Direct` Cyclesa z slotem `diffuse` pathon przy wyłączonych AO i GI), zapisana w `reference.json` i **nie dostrajana per obraz**. To zamyka niejasność „jasność Cyclesa bywa niższa o czynnik rzędu π” z [scenes.md](scenes.md).
- **Cycles:** Blender 5.2.2 LTS (wersja z [scenes.md](scenes.md)); stałe ziarno; 4096 próbek bez adaptacyjnego próbkowania; bez denoisingu (wariant z OIDN jako osobny plik, tylko do podglądu); odbicia łącznie 16 (diffuse 8, glossy 8, transmission 12); **kaustyki wyłączone** (nie mamy ich w czasie rzeczywistym); `clamp indirect` ustalony i zapisany w `reference.json`. Urządzenie (CPU albo konkretne GPU) i czas zapisane w metadanych.
- **Antyaliasing:** referencja renderowana w 2× (3840×2160) i uśredniana boxem do 1920×1080; pathon w dwóch trybach: `no-taa` (jedna klatka, porównanie z referencją 2×) i `taa-converged` (nieruchoma kamera, N = 64 klatek z jitterem).
- **Przebiegi Cyclesa** (do rozkładu błędu na składowe): Combined, Diffuse Direct i Indirect, Glossy Direct i Indirect, Transmission, Emission, Environment, AO, Depth, Normal, Cryptomatte (po materiale i obiekcie). Mapowanie na sloty pathon: `diffuse` i `specular` (ADR-0038, [slots.md](slots.md)).

### Metryki

| Metryka | Definicja | Co wychwytuje |
| --- | --- | --- |
| **FLIP** (NVIDIA ꟻLIP, LDR po wspólnym tonemapie i HDR-FLIP dla liniowego) | średnia i 95. percentyl mapy błędu | różnice postrzegane (krawędzie, kontrast, kolor); **licencja narzędzia do potwierdzenia** (zakładam BSD-3, nie sprawdzono); używamy jako zewnętrznego narzędzia dev, nie zależności crate'u |
| **SSIM** (okno gaussowskie 11×11, na luminancji po tonemapie) | średnia | struktura i cienie |
| **ΔE2000** (po tonemapie do sRGB) | średnia i 95. percentyl | wierność koloru |
| **Bias liniowy** | stosunek średnich luminancji pathon do Cyclesa per maska (słońce bezpośrednie, zacienione, okna, metal, drewno, szkło) | błędy energii (za jasno, za ciemno); maski z Cryptomatte |
| **RMSE względne** w pasie `Indirect Diffuse` | RMSE / średnia | jakość GI niezależnie od reszty |

Wspólny tonemap do metryk LDR: PBR Neutral (jest w `pathon-post`) zastosowany identycznie do obu obrazów w skrypcie komparatora, żeby różnica operatorów nie zaszła do wyniku.

### Skrypty i narzędzia (propozycje, nic nie jest zaimplementowane)

| Element | Opis |
| --- | --- |
| `tools/scenes/render_reference.py` | skrypt Blender CLI, w stylu `export_classroom.py`: `blender -b <plik> --python tools/scenes/render_reference.py -- --scene-json <plik> --camera <nazwa> --source blend\|glb --lighting lights-only\|ibl --samples 4096 --supersample 2 --out test-assets/reference/`; kroki: wczytaj źródło, ustaw kamerę z `scene.json`, odtwórz światła obszarowe z `extras.pathon_area` (dla `glb`), ustaw świat, ustaw Cycles i przebiegi, wyrenderuj, zapisz EXR wielowarstwowy i `reference.json` (wersja Blendera, urządzenie, ziarno, próbki, czas, stała ekspozycji) |
| Strona pathon | przykład `examples` (na bazie M4-10) zapisujący slot `hdr` (liniowy, z ekspozycją) do pliku float oraz osobno tonemapowany PNG z tym samym `exposure`; zmienna `PATHON_TEST_SCENE` jak w [scenes.md](scenes.md) |
| Komparator | zewnętrzny skrypt dev poza workspace (np. Python: `OpenEXR`, `flip-evaluator`, `numpy`), czytający EXR i obraz pathon, zapisujący `metrics.json` i kolaż porównawczy; **bez nowych zależności Cargo** (żeby nie ruszać `deps-check` i budżetu crate'ów) |
| Wyniki | `metrics.json` i kolaże PNG w `docs/bench/reference/<scena>/<kamera>/<wzorzec>/`; EXR w `test-assets/reference/` (poza gitem) |

### Użycie w pracy nad techniką

1. PR techniki dołącza tabelę ΔFLIP, ΔSSIM, ΔE i bias przed i po (wzorzec B, trzy kamery, dwa warianty oświetlenia).
2. Bramka regresji: żadna metryka nie pogarsza się o więcej niż 5% względnie bez uzasadnienia w PR. **Progi bezwzględne ustalamy po K01**, gdy znamy punkt startu (dziś nie ma żadnego pomiaru).
3. Golden images (ADR-0025) zostają osobną siatką regresji względem samego pathon; porównanie z Cyclesem jest ręczne lub nocne, nie w CI.
4. Każda technika z tabeli „Inwentarz” ma w opisie PR przypisane pasma Cyclesa, które powinna poprawić (np. GTAO → `AO` i `Indirect Diffuse` w zakamarkach).

## Ryzyka

**Techniczne**

- **Brak wzorca bez strat:** nawet wzorzec B ma straty eksportu (np. bump, korekcje koloru); metryki bezwzględne nie zbiegną do zera. Mierzymy kierunek zmian, nie „zero”.
- **Strojenie TAA, GI i wolumetryki** to największy koszt czasu; `taa.md` Filamenta skraca drogę dla TAA, dla pozostałych nie ma gotowego wzorca.
- **Pochodne ekranowe** w geometrycznym specular AA nie działają w ścieżce compute lub visibility buffer; materiał musi mieć wariant z pochodnymi (ADR-0030 zakłada „bez pochodnych” dla trafień RT).
- **Przeglądarka:** multi-draw-indirect do sprawdzenia (M4-11), opcjonalne `float32-filterable`, brak `SUBGROUP` w wgpu 30.0.2; techniki z listy G muszą mieć wariant bez nich.
- **Rozmiar paczek:** każdy kandydat to nowy crate; reguła `crate-budget` dotyczy domknięcia rdzenia, więc paczki jej nie obciążają, ale liczba crate'ów w `pathon` rośnie.

**Licencyjne** (szczegóły: [ADR-0047](adr/0047-uzywanie-kodu-filamenta.md))

- Pochodny plik zostaje Apache-2.0; crate z portem ma `license = "Apache-2.0"`, rdzeń zostaje `MIT OR Apache-2.0`. Punkt czeka na potwierdzenie autora.
- W nagłówku używamy linii `Copyright` ze źródła („The Android Open Source Project”), nie „Google LLC”.
- SSR, FXAA, AgX, GT7, OKLab, FSR 1, SGSR i część GTAO mają własne licencje w plikach Filamenta i nie są objęte zezwoleniem na kod Filamenta.
- `third_party/` Filamenta: zstd (GPL w wariancie), glslang (mieszane), clang (NCSA), modele CC BY-NC i bez zastosowań komercyjnych oraz pozostałe osobne projekty są poza zakresem (pełna tabela w ADR-0047).
- Tablice LTC (K05) i narzędzie FLIP (K01) to osobne źródła; licencje do potwierdzenia przed użyciem.
