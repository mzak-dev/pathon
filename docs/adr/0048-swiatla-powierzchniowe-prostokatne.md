# ADR-0048: Prostokątne światła powierzchniowe (LTC, własna tablica)

**Status:** Przyjęta
**Data:** 2026-11-26
**Dotyczy:** K05 ([filament-reference.md](../filament-reference.md)), [ADR-0042](0042-pbr-prepass-klastry-i-forward.md) (klastry i forward), [ADR-0047](0047-uzywanie-kodu-filamenta.md) (bez kodu Filamenta), [docs/scenes.md](../scenes.md) (eksport świateł), [docs/porownanie-obrazu.md](../porownanie-obrazu.md), kod diagnostyki `E0001`

## Kontekst

Pomiar K01 ([porownanie-obrazu.md](../porownanie-obrazu.md)) pokazał, że pathon rysuje prostokątne światła obszarowe sceny classroom jako półsferyczne spoty (to, co zapisuje eksporter): obraz wychodzi o 6 do 78% jaśniejszy niż wzorzec Cycles. Spot ma na osi natężenie P/(2π), a prostokąt Lamberta P/π, ale świeci jednakowo pod dużymi kątami, a prostokąt słabnie z kosinusem. Potrzebny jest prawdziwy prostokąt: poprawna jasność, miękkie oświetlenie i odblask w kształcie lampy.

## Decyzja

### 1. Model i jednostki

`LightKind::Area { width, height, two_sided }` (enum jest `non_exhaustive`): prostokąt w lokalnej płaszczyźnie XY, `width` wzdłuż X, `height` wzdłuż Y, emituje w stronę lokalnego −Z (jak reszta świateł); `two_sided` dodaje stronę +Z. `Light::intensity` to **moc świetlna całego światła w lumenach**.

Radiometria. Emiter Lamberta o stałej luminancji L na polu A ma wyjściowe wypromieniowanie M = πL, więc moc strony to P = πLA, czyli

- **L = P / (π·A)** (nity, cd/m²); przy `two_sided` moc dzieli się po równo na strony, L = P / (2π·A);
- natężenie na osi I(0) = L·A = **P/π** (kandela), a pod kątem θ od normalnej I(θ) = (P/π)·cos θ;
- natężenie oświetlenia na osi w odległości d (d ≫ rozmiar) E = P/(π d²).

Zgodność z eksportem: eksporter zapisuje światło Area o mocy W jako spot o natężeniu `W/(2π)·683` cd ([scenes.md](../scenes.md)), czyli strumień półsfery P = 683·W lm = 2π · (kandele spota). Loader wylicza więc moc prostokąta jako **kandele ze spota · 2π** (nie zakłada stałej 683); rozmiar bierze z `extras.pathon_area` (`size_x`, `size_y`; dysk i elipsa zamieniane na prostokąt o tym samym polu i proporcjach), a skala węzła rozciąga prostokąt. Przy takiej konwencji prostokąt ma na osi P/π = 2× natężenie dawnego spota i słabnie z kosinusem, co jest zachowaniem Cyclesa zmierzonym w K01.

W shaderze `GpuLight` (96 bajtów, dwa dodatkowe `vec4` z połówkami boków) niesie `kolor · L`. Zasięg do klastrów: obszar nigdy nie oświetla punktu silniej niż I(0)/d², gdzie d to odległość do prostokąta (współczynnik kształtu ≤ A/(πd²)), więc `influence_radius` = √(I(0)·kolor_max / `AUTO_RANGE_CUTOFF_LUX`) **+ połowa przekątnej**; kula zasięgu wokół środka obejmuje każdy punkt, do którego światło dociera, także dla wielkich prostokątów. Okno `saturate(1 − (d/r)⁴)²` (jak dla punktowych) liczone od środka sprawia, że światło znika gładko na zasięgu. Jawny zasięg krótszy niż połowa przekątnej ucina prostokąt: ostrzega `E0001`.

### 2. Cieniowanie

**Diffuse: dokładnie, bez tablic.** Całka Lamberta z wielokąta sferycznego po obcięciu do horyzontu normalnej: ∫cos θ dω = ½ Σ θᵢ (γᵢ·n) po krawędziach (Arvo 1995, „Applications of Irradiance Tensors to the Simulation of Non-Lambertian Phenomena”). Wynik razy L i razy `front_diffuse` (to samo BRDF co dla światła punktowego, z Fresnelem liczonym ku środkowi prostokąta). Zgodność z całkowaniem numerycznym: 0,1% (granica błędu `f16` w odczycie).

**Specular: LTC** (Heitz, Dupuy, Hill i Neubelt 2016, „Real-Time Polygonal-Light Shading with Linearly Transformed Cosines”). Rozkład GGX·cos jest przybliżony transformowanym kosinusem `D(l) = D_o(M⁻¹l/|M⁻¹l|)·|det M⁻¹|/|M⁻¹l|³`; całka po prostokącie to całka kosinusa po prostokącie przekształconym przez M⁻¹ (po obcięciu do horyzontu, to samo co w diffuse). Skala: całka lobu razy Fresnel Schlicka rozbita na `f0·(∫DV·n.l − ∫(1−v.h)⁵DV·n.l) + f90·∫(1−v.h)⁵DV·n.l`, więc tablica nie zależy od IBL (bez `IblPack` tablica LUT środowiska jest czarna). Clear coat: drugi lob z chropowatością powłoki i normalną geometryczną; transmisja i subsurface jak w `evaluate`. Cieni światła powierzchniowe nie mają (hak `shadow_factor` działa jak dla innych świateł).

### 3. Tablica LTC: własny fit, bez cudzych danych

Repozytorium `selfshadow/ltc_code` (autorzy artykułu) ma plik `LICENSE` w stylu BSD-3 z prośbą o cytowanie artykułu (odczytane 26.11.2026 przez podgląd strony; nie jest to porada prawna), ale żaden zapis nie mówi o samych tablicach, a nie mamy pewności co do ich statusu. Zgodnie z poleceniem zadania **nie kopiujemy niczego**: tablicę generuje własne narzędzie [`tools/pathon-ltc-fit`](../../tools/pathon-ltc-fit/src/lib.rs), napisane z samego artykułu.

- Parametryzacja: `M = [[m11,0,m13],[0,m22,0],[m31,0,1]]` (izotropowe BRDF w ramce, w której widok leży w płaszczyźnie xz); w tablicy odwrotność znormalizowana tak, by środkowy element wynosił 1 (cztery liczby `a, b, c, d`).
- Dopasowanie: Nelder-Mead po `(ln m11, ln m22, m13, m31)`, błąd = odległość L1 `|D_brdf − D_ltc|` (w naszych próbach niższa wariacja całkowita niż dla błędu L3) liczona estymatorem MIS na próbkach rozłożonych równomiernie z obu rozkładów (GGX przez próbkowanie wektora połówkowego, LTC przez kosinus), start z sąsiedniego węzła; restarty do braku poprawy.
- Siatka 32×32: chropowatość percepcyjna `r` równomiernie (`α = max(r², 0,0015)`), widok równomiernie w `√(1 − n·v)`; interpolacja dwuliniowa w shaderze. Dane: [`ltc_table.rs`](../../crates/pathon-pbr/src/ltc_table.rs) (1024 węzły po 8 liczb `f32`, 32 KB, w pamięci karty jako bufor storage dołączany tylko w permutacji z flagą `area_lights`, binding 13). Powtórzenie: `cargo run -p pathon-ltc-fit --release` (ok. 30 s).
- Jakość (wariacja całkowita lobu BRDF względem dopasowania, 0 = identyczne): średnia 0,036; ≤ 0,03 dla widoku bliższego normalnej niż 45° i każdej chropowatości; do 0,10 do 0,16 przy `n·v` ≈ 0,2 i chropowatości 0,3 do 0,6; powyżej 0,3 przy widoku prawie stycznym. Ogon lobu przy widoku stycznym jest przeszacowany (rodzina LTC nie odtwarza „półksiężyca” GGX), co widać jako delikatny połysk z tyłu odblasku.

### 4. Granice

Brak cieni, brak tekstury światła (kolor stały), brak dysku i sfery (dysk jako prostokąt o tym samym polu), brak profili IES. Specular nie uwzględnia kompensacji wielokrotnego rozpraszania (jak światła punktowe w obecnym `evaluate`). Konwersja loadera zakłada eksport ze `scenes.md`.

## Konsekwencje

- Nowa permutacja koloru `area_lights` (jedna na scenę, tylko gdy są światła powierzchniowe); sceny bez takich świateł mają ten sam shader co przedtem. `GpuLight` rośnie z 64 do 96 bajtów.
- Loader i viewer: `--area-as-spot` przywraca spoty z pliku (porównania przed i po). Tryb `--simple` viewera nadal rysuje te spoty.
- Dokładność specular jest ograniczona jakością dopasowania (punkt 3); poprawa (węzły gęściej przy widoku stycznym, rozszerzenie rodziny, przycięcie do horyzontu powierzchni) to osobna praca i nie zmienia formatu użycia tablicy.

## Testy

`crates/pathon-pbr/tests/area_lights.rs` i testy jednostkowe `area.rs`, `ltc.rs`, `light.rs`, `tools/pathon-ltc-fit`: diffuse analityczny kontra kwadratura (0,01%) i kontra GPU (0,1%); światło daleko (10× rozmiar) kontra punktowe źródło o natężeniu (P/π)cos θ (błąd < 2%); specular LTC kontra źródło punktowe w lobie (≤ 15%, z listą błędów w logu) i kontra całkowanie numeryczne GGX po prostokącie na GPU (średnio 1,4 do 7,4% tego, co widzi piksel, najgorszy piksel do 53% przy wąskim odblasku i widoku 80° od normalnej); dwustronność z podwójną mocą daje to samo co jedna strona; światło za powierzchnią, odwrócone i pod podłogą nic nie dodaje; prostokąty przecinające powierzchnię, leżące w niej i ogromne bez NaN; 100 świateł w klastrach kontra brute force (CPU i GPU); golden `tests/golden/pbr_area_light.png`.
