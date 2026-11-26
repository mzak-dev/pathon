# ADR-0044: Cienie kaskadowe (CSM) i bias głębi w RHI

**Status:** Przyjęta
**Data:** 2026-11-25
**Dotyczy:** M4-07 (`pathon-shadow`), `pathon-rhi-types` (`DepthBias`), [ADR-0019](0019-pipeline-cache-i-zrodlo-shaderow.md) (pola klucza potoku), [ADR-0033](0033-komendy-indirect.md), [ADR-0020](0020-kontrakty-rhi-types.md)

## Kontekst

Pierwsza paczka cieni ma dać cienie światła kierunkowego bez pływania krawędzi, bez acne i bez peter-panningu, na rdzeniu, który nie zna żadnej techniki (rola `Shadows`, etap `Stage::Shadows`). `pathon-pbr` (M4-05) powstaje równolegle i dostaje cienie przez funkcję WGSL, więc kontrakt wejścia i wyjścia musi być jawny. ADR-0019 zapowiadał, że bias głębi dojdzie do potoku razem z pierwszym użytkownikiem; to ten.

Technika jest napisana z publikacji (podział kaskad: Zhang i in. 2006, Dimitrov 2007; stabilizacja: Valient 2008; normal offset: Holbert 2011; szum: Jimenez 2014), bez kodu cudzych silników ([ADR-0047](0047-uzywanie-kodu-filamenta.md)).

## Decyzja

**RHI.** `DepthState` dostaje pole `bias: DepthBias { constant: i32, slope_scale, clamp }` (domyślnie zero; `DepthState::reverse_z` bez zmian). Liczby `f32` są trzymane jako bity, więc opis potoku zostaje `Eq + Hash` i bias wchodzi do klucza potoku sam. Znak jest znakiem bufora głębi: przy reverse-Z odsunięcie od obserwatora to wartości ujemne. `rhi-wgpu` przekazuje go do `wgpu::DepthBiasState`; `rhi-null` odrzuca nieskończone `slope_scale` / `clamp` i format koloru w `DepthState` jako R0109. Test konformancji (null i GPU) sprawdza, że stały bias i bias od nachylenia przesuwają fragment w teście głębi, a bias w złą stronę nie.

**Kaskady.** Podział zakresu głębi kamery metodą praktyczną (mieszanka podziału równomiernego i logarytmicznego, `lambda`) albo ułamkami podanymi wprost; od 1 do 4 kaskad, ostatnia kończy się na `max_distance`. Kamera jest odczytywana z macierzy projekcji (perspektywa i ortho, reverse-Z, skończona i nieskończona płaszczyzna far), więc paczka nie potrzebuje `pathon-scene` po stronie kamery. Dwa dopasowania prostokąta ortho do plasterka frustum:

- `Stable` (domyślne): kula obejmująca plasterek liczona w przestrzeni kamery wyłącznie z obiektywu i dwóch odległości (nie zależy od pozycji ani kierunku kamery, więc ma te same bity w każdej klatce), środek przyciągnięty do siatki tekseli światła. Mapa przesuwa się wyłącznie o całe teksele, więc krawędź cienia nie pływa. Test GPU czyta mapę głębi dla pięciu ruchów kamery i sprawdza, że jest tym samym obrazem przesuniętym o całkowitą liczbę tekseli.
- `Focused`: ciasne pudełko w przestrzeni światła; do dwóch razy większa rozdzielczość na szerokim, płaskim plasterku, ale krawędzie migoczą przy ruchu. Test pokazuje różnicę (kontrola negatywna).

Zakres głębi każdej kaskady sięga do najbliższego rzucającego cień, który zachodzi na jej prostokąt (rzucający bez znanych granic: o `unbounded_caster_distance` do przodu), a kończy na plasterku. Rzucający poza prostokątem kaskady nie są do niej rysowani (culling w przestrzeni światła).

**Głębia map.** Jedna tekstura `Depth32Float` z warstwą na kaskadę (slot `shadow.map`, reverse-Z: 1 przy świetle), jeden pass grafu `shadow cascades` w `Stage::Shadows` renderujący po warstwie przez widok `layer`; konsument czyta ją widokiem `D2Array` i deklaruje dostęp `SAMPLED | DEPTH_READ` (tak wymaga tekstura głębi, stała `SHADOW_MAP_ACCESS`). Sampler porównawczy `GreaterEqual` z filtrowaniem liniowym (każdy tap to już PCF 2×2).

**Bias** (trzy warstwy, każda z innej przyczyny):

1. Offset wielokąta w passie głębi (`DepthBias`, stały i od nachylenia, znak odwrócony dla reverse-Z): chroni przed acne wynikającym z nachylenia powierzchni względem światła.
2. Przesunięcie rzucających cień o `constant_bias` metrów od światła (zapieczone w macierzy passu głębi, nie w macierzy próbkowania): chroni przed acne na powierzchniach prostopadłych do światła.
3. Normal offset po stronie odbiorcy: pozycja jest przesuwana wzdłuż normalnej o `sin(theta) * normal_bias * rozmiar_teksela_kaskady`, gdzie `theta` to kąt do światła; rośnie tam, gdzie powierzchnia jest widoczna pod kątem. Bias w jednostkach tekseli skaluje się sam z kaskadą.

**Filtr.** `Hard` (1 tap), `Pcf3` (domyślny: filtr pudełkowy o szerokości 3 tekseli w 4 tapach sprzętowych, wagi z nakładania pudełka na teksele, bez szumu), `Pcf5` (16 próbek dysku Poissona o promieniu 2,5 tekseli, opcjonalnie obracany szumem interleaved gradient noise). Przejście między kaskadami jest mieszane na ostatnich `blend_fraction` zakresu, a cień zanika na ostatnich `fade_fraction` `max_distance`.

**Rysowanie rzucających cień.** Wejście to `ShadowScene` w `Shared` (światło i lista `ShadowCaster`: bufory pozycji i indeksów z chunka `GeometryPool`, zakres indeksów, `base_vertex`, macierz modelu, pudełko świata); `fill_from_world` buduje ją z `RenderWorld` i puli. Macierze modeli idą do jednego bufora storage, a rysowania do mapy: gdy urządzenie ma `Features::INDIRECT_FIRST_INSTANCE`, jeden `multi_draw_indexed_indirect` na grupę buforów w kaskadzie (komendy z ADR-0033, argumenty `DrawIndexedIndirectArgs` z `first_instance` = slot macierzy), inaczej pętla `draw_indexed` z zakresem instancji. Oba wyniki są identyczne co do bitu (test GPU). Culling GPU (M4-11) zastąpi budowanie argumentów po CPU.

**Wyjście dla oświetlenia.** Slot `SHADOW_MAP`, `ShadowFrame` w `Shared` (blok uniform `ShadowUniforms`, 368 B, z miejscem w pierścieniu uniformów tej klatki, oraz sampler) i `ShadowBindings` (deklaracje WGSL, wpisy layoutu, wpisy bind groupy; trzy kolejne bindingi). Moduł `package::shadow::sample` (WESL, bez własnych bindingów jak `ibl::sample`) daje `shadow_factor_directional(shadows, map, sampler, light_index, world_position, normal, view_depth, pixel) -> f32`: 0 w cieniu, 1 oświetlone; światło o innym indeksie i pozycje za `max_distance` są oświetlone. `pathon-pbr` podpina to za swoim hakiem `shadow_factor(light, position)`: normalną, głębię widoku i `@builtin(position).xy` musi mieć pod ręką (np. w zmiennych prywatnych shadera). Gdy nie ma światła w `ShadowScene`, nie ma slotu ani `ShadowFrame`; konsument używa wtedy permutacji bez cieni (flaga `shadows`, ADR-0023).

**Granice.** Paczka zależy od `pathon-api` oraz `pathon-mesh` i `pathon-scene` (`pack-lib`, ADR-0041); nie zmienia rdzenia. Diagnostyki `D0001`…`D0003` (ustawienia naprawione, kierunek światła, projekcja kamery).

## Odłożone (z uzasadnieniem)

- **Światła punktowe i reflektory (kostka / perspektywa).** *Zrobione w [ADR-0050](0050-cienie-swiatel-punktowych-i-reflektorow.md) (atlas map perspektywicznych zamiast kostki).* Pierwotny opis: `ShadowScene` ma jedno światło, ale `light_index` w uniformie i w funkcji WGSL oraz tablica warstw zostawiają miejsce na kilka świateł; kostki i perspektywa wymagają cube / `CubeArray` w grafie (ADR-0020, aneks: graf nie ma cube'a) i osobnego budżetu pamięci. Zadanie osobne, gdy pbr będzie miał pętlę po światłach.
- **Miękkie cienie zmienne (PCSS, Fernando 2005) i warianty wariancyjne (VSM / EVSM).** Wymagają wyszukiwania zasłaniających i mipów albo rozmycia momentów w `Rgba32Float`; `Pcf5` daje stałą miękkość. Wrócimy po M4-17 z pomiarem kosztu.
- **Cienie kontaktowe w przestrzeni ekranu.** Marsz promienia po slocie `depth` zamyka szczeliny, których nie zamyka mapa o skończonej rozdzielczości; to funkcja po stronie oświetlenia (pbr / SSAO), nie paczki cieni.
- **Perspektywiczne warpowanie map (LiSPSM / TSM).** Nie łączy się ze stabilizacją, którą wybraliśmy; kaskady dają większość zysku.
- **Przezroczyste rzucające cień (alpha test).** Pass głębi nie ma fragment shadera; materiał z maską alfa dostanie wariant w M4-15.

## Konsekwencje

- `DepthState` dostało pole: literały `DepthState { .. }` w kodzie użytkowników trzeba uzupełnić (w repo wszyscy używają `reverse_z`).
- `R0109` obejmuje teraz też niepoprawny stan głębi potoku.
- Zmiana `ShadowUniforms` to zmiana kontraktu z pbr: rozmiar i układ pilnuje test (368 bajtów, układ WGSL `ShadowCascades`).
- Referencja `shadows_csm` powstała na Vulkanie (RX 9070 XT); lavapipe, WARP i Metal dopiero w CI. Krawędzie PCF różnią się między rasteryzatorami w progu `cross_backend`.
