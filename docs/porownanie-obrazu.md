# Porównanie obrazu pathon z Cyclesem (K01)

Stan na 25.11.2026. Protokół mierzenia, jak daleko obraz pathon jest od ścieżkowego renderu Blendera tej samej sceny. Opis scen: [scenes.md](scenes.md); uzasadnienie i kontekst (wzorce A i B, lista technik): [filament-reference.md](filament-reference.md#protokół-porównania-z-cyclesem-scena-classroom). Tu jest część praktyczna: polecenia, ustawienia, jak czytać liczby i pierwsze wyniki wzorca.

Zrobione w K01: render wzorca w Blenderze (`tools/scenes/render_reference.py`) i komparator (`tools/scenes/compare.py`). **Nie zrobione:** strona pathon (przykład zapisujący `hdr` do pliku, patrz [Kontrakt strony pathon](#kontrakt-strony-pathon)), więc **żadnego pomiaru pathon jeszcze nie ma**; progi poniżej są propozycją opartą na obrazach pomocniczych, nie na wynikach renderera.

## Wymagania

| Element | Wersja / uwagi |
| --- | --- |
| Blender | 5.2.2 LTS, `C:\Program Files\Blender Foundation\Blender 5.2\blender.exe` (katalog „Blender 4.5” nie ma `blender.exe`). API 5.x: obraz wielowarstwowy ustawia się przez `image_settings.media_type`, nie przez `file_format` |
| GPU | AMD Radeon RX 9070 XT przez HIP z HIP RT (skrypt wybiera HIP, potem OptiX, CUDA, oneAPI, na końcu CPU). Na tej maszynie jest też GTX 1650 SUPER (CUDA): jest wyłączona, żeby nie mieszać urządzeń |
| Python | 3.11 z `numpy` i `Pillow` (wystarczą do `compare.py`; Blender używa własnego Pythona) |
| FLIP (opcjonalnie) | `pip install flip-evaluator` w venv (BSD-3-Clause, [NVlabs/flip](https://github.com/NVlabs/flip), sprawdzone 25.11.2026: LICENSE „BSD 3-Clause”, klasyfikator PyPI „BSD License”). `compare.py` używa go, jeśli jest zainstalowany, inaczej pomija pole `flip`. Nie jest zależnością Cargo ani nie wchodzi do repozytorium |

Nic nie instalujemy globalnie; do FLIP wystarczy `python -m venv --system-site-packages .venv-flip && .venv-flip/Scripts/pip install flip-evaluator` poza repozytorium.

## Krok 1: render wzorca

Wzorzec B z [filament-reference.md](filament-reference.md): **ten sam `classroom.glb`, który wczytuje pathon**, zaimportowany do pustej sceny Blendera (importer glTF), więc różni się tylko renderer, nie zawartość. Źródłem jest wyeksportowany plik, nie `classroom.blend`, żeby nie mieszać strat eksportu ([scenes.md](scenes.md#materiały-principled-bsdf-i-straty)) z różnicami renderera.

```sh
blender -b --factory-startup --python tools/scenes/render_reference.py -- test-assets/classroom/reference \
  --glb test-assets/classroom/classroom.glb --samples 4096 --calibrate-exposure
```

(`--factory-startup` i pusty plik są potrzebne, bo skrypt sam czyści scenę; nie trzeba podawać `.blend`.) Opcje:

| Opcja | Domyślnie | Znaczenie |
| --- | --- | --- |
| `--cameras` | `renderCam,pathon_wide,pathon_reverse` | nazwy obiektów kamer z `.glb` |
| `--lighting lights-only\|ibl\|both` | `both` | `lights-only`: świat czarny (jak w oryginale), `ibl`: świat z `env/Garage.hdr` |
| `--samples` | 512 | próbki na piksel; wzorzec zapisany tu to 4096 |
| `--denoise off\|on\|both` | `both` | `off`: tylko surowy render (z AOV), `on`: tylko OpenImageDenoise, `both`: dwa rendery na kamerę |
| `--resolution` | `960x540` | rozdzielczość; to samo co zrzuty pathon (`--size 960x540`) |
| `--area-lights real\|spot` | `real` | `real`: prostokątne światła obszarowe z `extras.pathon_area`; `spot`: półsferyczne spoty z `.glb`, czyli to, co dziś rysuje pathon |
| `--device auto\|hip\|optix\|cuda\|cpu` | `auto` | urządzenie Cyclesa |
| `--clamp-indirect` | 0 | 0 wyłącza obcinanie, patrz [Ustawienia](#ustawienia-wzorca) |
| `--exposure-ev` | 0 | tylko pliki PNG; EXR zawsze surowy |
| `--ibl-strength` | 1 | siła świata w wariancie `ibl` |
| `--no-aovs` | wyłączone | bez dodatkowych przebiegów |
| `--calibrate-exposure` | wyłączone | najpierw zmierz radiancję białej płaszczyzny pod słońcem i zapisz do `reference.json` |

Wynik w `<out_dir>/<lighting>/`: `<kamera>.exr` (wielowarstwowy, liniowy Rec.709, float32, bez odszumiania, z AOV), `<kamera>.png` (ten sam obraz jako sRGB, transformacja Standard), `<kamera>_denoised.exr` i `.png`; w `<out_dir>/reference.json` komplet ustawień (wersja Blendera, urządzenie, próbki, ziarno, ścieżki światła, kamery, światła po imporcie, mapowanie materiałów na indeksy, czasy, suma SHA-256 `.glb`). Katalog jest w `test-assets/` (poza gitem; ok. 45 MB na plik z AOV).

### Ustawienia wzorca

| Ustawienie | Wartość | Dlaczego |
| --- | --- | --- |
| Silnik | Cycles, GPU (HIP RT), 4096 próbek, **bez** adaptacyjnego próbkowania, ziarno 0 stałe | powtarzalność; 4096 jak w protokole |
| Odbicia | łącznie 16, diffuse 8, glossy 8, transmisja 12, przezroczystość 16, objętość 0 | z protokołu |
| Kaustyki | wyłączone (odbite i załamane) | pathon ich nie ma |
| Obcinanie indirect (`clamp indirect`) | **wyłączone** | domyślne 10 w Blenderze **zaniża średnią jasność o ok. 21%** (zmierzone na `renderCam`, 512 próbek: 0,257 przy 10, 0,322 przy 30, 0,326 przy 100 i przy 0); obcinanie ucina światło z silnych, małych źródeł (lampa za oknem o 213 437 cd), czyli właśnie to, co mierzymy. Cena: surowy obraz ma świetliki, dlatego strukturę mierzymy na wersji odszumionej |
| Filtr piksela | Box, szerokość 1 px | piksel to zwykła średnia z jego pola, jak w zrzucie pathon z `--supersample` (zamiast renderu w 2× i uśredniania) |
| Rozdzielczość | 960×540 | jak zrzuty z `docs/scenes/`; protokół w filament-reference mówił o 1920×1080 z referencji 2×, to jest cenowo lżejsza zamiana |
| Kolor | transformacja widoku `Standard`, look `None`, ekspozycja 0, gamma 1, wyświetlacz sRGB; EXR liniowy | bez Filmic/AgX |
| Kamery | obiekty z `.glb` (pozycja, orientacja, `yfov` jak w pathon; zgodność `yfov` sprawdzana w `reference.json`) | `renderCam` w `.glb` leży o 2 mm (x) i 7 mm (z) od pozycji z `*.scene.json`; używamy `.glb`, bo tak robi loader pathon |
| Światła | importer glTF w trybie SPEC (odwrotność eksportu: lux → W/m², kandela → W przez 683), promień 0, więc ostre cienie jak w pathon; prostokąty obszarowe z `extras.pathon_area` | patrz [Jednostki](#jednostki-i-stała-kalibracyjna) |
| Świat `lights-only` | czarny, siła 0 | jak oryginał |
| Świat `ibl` | `Garage.hdr` (równoprostokątny, 2000×1000), siła 1, filtrowanie liniowe, **obrót Z = −90°** | patrz niżej |
| Odszumianie | OpenImageDenoise na GPU, tylko dla plików `*_denoised.*` | `.exr` bez przyrostka jest zawsze surowy |

Obrót świata: pathon mapuje środek obrazu równoprostokątnego na kierunek −Z (glTF), z `u` rosnącym ku +X ([pathon-ibl](crates/pathon-ibl.md), `equirect_direction`). Cycles ma środek obrazu na +X (Blender, Z w górę). Po konwersji osi to przesunięcie o 90°; znak sprawdzony pomiarem: kamera 0,5° patrząca w cztery kierunki zwraca tę samą radiancję co teksel `Garage.hdr` w odpowiednim `u` (środek, `u` = 0,25, 0,75, 0). Wartości z `Garage.hdr`: średnia luminancja 0,148, maksimum 78,8 (mały jasny obszar), jest więc ciemny i ma niewielki wkład w tej scenie (patrz niżej).

Okna: obiekty `dayLight_portal` zostały pominięte przy eksporcie, więc w wariancie `lights-only` okna pokazują czerń (otwory w ścianie), a w `ibl` widać przez nie `Garage.hdr`.

### Jednostki i stała kalibracyjna

- **Słońce:** 15 W/m² w Blenderze to 10 245 lx w glTF (15 · 683). Zmierzone w Blenderze 5.2.2: biała powierzchnia Lambert o albedo 1 pod słońcem 15 W/m² daje radiancję 4,7746 = 15/π (stosunek zmierzone/oczekiwane 1,000000).
- **Światło punktowe:** P watów w Blenderze to P/(4π) W/sr (izotropowo, bez dodatkowego π); zmierzone na płaszczyźnie: `L = P/(4π²h²)`, czyli `E = P/(4π h²)`. glTF: kandela = P/(4π)·683, więc po ekspozycji 1/683 jednostki są te same.
- **Światło obszarowe:** prostokąt o mocy P jednostronny Lambert, natężenie osiowe P/π W/sr (zmierzone). Spot z `.glb` (półsferyczny, strumień zachowany) ma P/(2π), czyli **o połowę mniej na osi**; to znana różnica pathon przed K05 (światła powierzchniowe).
- **Stała kalibracyjna: K = 1, ekspozycja pathon 1/683.** `radiancja_Blender = radiancja_pathon_przed_ekspozycją / 683`. Wyznaczona raz (`--calibrate-exposure` zapisuje pomiar do `reference.json`) i **nie dostrajana per obraz**. Emisja materiałów: Blender podaje ją w W/m²/sr jako radiancję, glTF w nitach (cd/m²), więc pathon musi traktować `emissive · emissive_strength` z mnożnikiem 683 przed ekspozycją (tak robi viewer). Rozbieżność „czynnik rzędu π” z [scenes.md](scenes.md) **nie występuje**: konwencje eksportu i importu są odwrotne, światła zgadzają się co do jednostki.

## Kontrakt strony pathon

Strony pathon jeszcze nie ma. Żeby `compare.py` mógł ją porównać, przykład w `examples/` (na bazie M4-10) ma zapisać:

| Wymaganie | Wartość |
| --- | --- |
| Plik | `<kamera>.npy` (float32, kształt `[540, 960, 3]`, wiersz 0 u góry, liniowy Rec.709, **bez tonemappingu**, ekspozycja 1/683 zastosowana); alternatywnie Radiance `.hdr` albo `.exr` |
| Kamera | ta z `.glb` o danej nazwie; `main` ma tylko `renderCam` |
| Wariant `lights-only` | bez ambientu i bez IBL (viewer ma dziś ambient 400/150 lx i emisję ×683: wyłączyć ambient) |
| Wariant `ibl` | `Garage.hdr`, siła 1, bez obrotu (konwencja pathon-ibl) |
| Antyaliasing | `no-taa`: jedna klatka z `--supersample 2`; `taa-converged` dopiero z K06 |
| Cienie, ekspozycja, tonemap | tonemap robi komparator (PBR Neutral); pathon zapisuje liniowe |

## Krok 2: porównanie

```sh
python tools/scenes/compare.py \
  --reference test-assets/classroom/reference/lights-only/renderCam_denoised.exr \
  --test wyniki/pathon_renderCam.npy --out-dir wyniki/renderCam --name renderCam
```

- Jako `--reference` użyj pliku `*_denoised.exr`: struktura (SSIM, FLIP) przy surowym renderze liczy też szum Cyclesa. Maski i udziały AOV `compare.py` bierze automatycznie z surowego `<kamera>.exr` obok, a mapowanie materiałów z `reference.json` powyżej w drzewie katalogów (`--reference-json` wymusza inne).
- `--test-scale` mnoży radiancję pathon (jedna stała z kalibracji, nie per obraz), `--exposure` mnoży oba obrazy przed tonemappingiem.
- `--synthetic direct-only|direct-flat-ambient` porównuje wzorzec z podzbiorem jego własnych AOV (patrz niżej); służy do zakotwiczenia progów i jako test narzędzia.
- Wyniki: `<nazwa>_metrics.json`, `<nazwa>_collage.png` (referencja | test | mapa ΔE2000 | stosunek luminancji w EV, czerwony = test jaśniejszy, niebieski = ciemniejszy, ±2 EV), `<nazwa>_delta_e.png`.

Czyta EXR bez `OpenEXR` (własny czytnik: skanowane linie, bez kompresji, ZIP, ZIPS; półprecyzja i float; wielowarstwowe, część wielu obrazów). Inne kompresje (PIZ, DWAA) zgłasza błędem.

### Metryki (co liczy `compare.py`)

Oba obrazy przechodzą ten sam tonemap **PBR Neutral** (Khronos) i kodowanie sRGB, żeby różnica operatorów nie weszła do wyniku.

| Metryka | Definicja | Jak czytać |
| --- | --- | --- |
| `psnr_db` | PSNR na obrazie po tonemapie, szczyt 1 | ogólna odległość; mało czuły na strukturę |
| `ssim` | SSIM, okno gaussowskie 11×11 σ = 1,5, na luminancji po tonemapie | struktura, cienie; **szum referencji go obniża** |
| `delta_e2000_mean` / `_p95` | CIEDE2000 po tonemapie do sRGB | wierność koloru i jasności; p95 łapie duże błędy lokalne (brak cienia, brak odbicia) |
| `flip` | NVIDIA ꟻLIP: `ldr_mean`, `ldr_p95` na obrazie po tonemapie, `hdr_mean`, `hdr_p95` na liniowym | błąd „postrzegany”; 0 = identyczne, 1 = skrajnie różne |
| `bias_total` | średnia luminancja liniowa pathon / wzorzec | błąd energii: < 1 za ciemno |
| `per_mask.<maska>.bias_linear`, `bias_ev`, `delta_e2000_mean` | to samo w masce | gdzie jest błąd; maski: `board`, `floor`, `windows` (ramy i żaluzje), `walls_ceiling`, `wood_furniture`, `metal`, `plastic_desk_tops` z indeksu materiału, oraz `direct_lit`, `direct_shadowed` (połowa pikseli o większym i mniejszym Diffuse Direct), `emissive` |
| `reference_energy_share` | udział terminów Cyclesa w luminancji obrazu | patrz [Co zawiera różnicę](#co-zawiera-różnicę) |

Nie ma jeszcze metryki „RMSE względne w paśmie Indirect Diffuse” z protokołu: wymaga osobnego przebiegu oświetlenia pośredniego po stronie pathon.

**Maski bez Cryptomatte.** Przebiegi Cryptomatte w EXR wymagają osobnego dekodera; zamiast tego każdy materiał dostaje stały `pass_index` (kolejność alfabetyczna nazw, mapa w `reference.json`), a przebieg Material Index daje maskę z dokładnością do piksela (piksel na styku dwóch materiałów ma uśrednioną, niecałkowitą wartość indeksu i wypada z maski).

## Pierwsze wyniki wzorca

Render 25.11.2026, Blender 5.2.2 LTS, RX 9070 XT (HIP RT), 4096 próbek, rozdzielczość 960×540, 12 renderów (3 kamery × 2 warianty światła × surowy i odszumiony). Jeden render surowy trwał 104 do 162 s, odszumiony 86 do 144 s (odszumiony to osobny render z tą samą liczbą próbek; GPU bywało współdzielone z innymi zadaniami), całość 22 minuty; import `.glb` 4,5 s; do szybkich prób wystarcza `--samples 512` (ok. 18 s na obraz) albo 16 próbek (12 s, razem z kompilacją jądra). `reference-spot` (1024 próbki, 12 renderów) trwał ok. 5 minut. Pliki: `test-assets/classroom/reference/` (wzorzec z prawdziwymi światłami obszarowymi) i `test-assets/classroom/reference-spot/` (1024 próbki, **te same półsferyczne spoty, które dziś rysuje pathon**; patrz niżej, dlaczego są dwa). Małe PNG w [scenes/reference/](scenes/reference/): `renderCam-lights-only.png`, `pathon_wide-lights-only.png`, `pathon_reverse-lights-only.png` (odszumione, 960×540) oraz `renderCam-direct-only.png` i `renderCam-indirect-only.png` (rozkład na światło bezpośrednie i pośrednie, surowe 4096 próbek).

![renderCam, Cycles](scenes/reference/renderCam-lights-only.png)

### Co zawiera różnicę

Rozkład luminancji obrazu wzorca na terminy Cyclesa: `Combined = kolor · (bezpośrednie + pośrednie)` dla każdego płata (diffuse, glossy, transmisja) plus emisja i środowisko (przebiegi `Diffuse/Glossy/Transmission Direct/Indirect/Color`, `Emission`, `Environment`); suma terminów odtwarza `Combined`. Wartości w % luminancji obrazu, wariant `lights-only`:

| Kamera | Diffuse bezpośrednie | **Diffuse pośrednie** | Glossy bezpośrednie | Glossy pośrednie | Transmisja pośrednia | Emisja | Środowisko | **Razem pośrednie** |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `renderCam` | 35,3 | **50,6** | 3,7 | 4,1 | 0,4 | 6,0 | 0,0 | **55,1** |
| `pathon_wide` | 28,2 | **56,3** | 4,0 | 4,3 | 0,1 | 7,0 | 0,0 | **60,8** |
| `pathon_reverse` | 28,3 | **59,4** | 2,6 | 4,9 | 0,0 | 4,8 | 0,0 | **64,3** |

Wariant `ibl` różni się od `lights-only` o 0 do 4 punktów procentowych (środowisko 0,1 / 2,9 / 0,6%): `Garage.hdr` jest ciemny, a okna małe, więc w tej scenie IBL niemal nic nie dodaje (luminancja `ibl` względem `lights-only`: 1,006 / 1,035 / 1,007, ΔE2000 średnie 0,4 do 0,7, SSIM powyżej 0,98). **Wariant `ibl` słabo testuje IBL w classroom**; do tego trzeba innej sceny albo jaśniejszego HDR.

**Wniosek: classroom to wnętrze oświetlone w ponad połowie światłem odbitym.** 55 do 64% luminancji obrazu pochodzi z oświetlenia pośredniego, z czego niemal wszystko (51 do 59 punktów) to diffuse. W zacienionej połowie pikseli (poniżej mediany `Diffuse Direct`) światło bezpośrednie daje tylko 12 do 21% luminancji; w oświetlonej połowie wciąż 52 do 57%. Ramy okien i żaluzje (`windows`) mają 85 do 92% światła pośredniego, prawdopodobnie dlatego, że od środka widać ich stronę osłoniętą przed lampą za oknem (nie sprawdzano osobno). Oglądając `renderCam-direct-only.png` i `renderCam-indirect-only.png`: sam obraz bezpośredni to ostre plamy słońca i światło tablicy na czarnym tle, a pośredni nadaje pokojowi wygląd: jasna ściana przy oknach, wnętrze ławek i tablica odbijająca światło. Bez GI pokój wygląda jak scena nocna z latarką.

Żeby skwantyfikować, ile daje „prawie GI” i ile tylko jego brak, `compare.py` umie porównać wzorzec z jego własnym podzbiorem AOV (obrazy pomocnicze, bez pathon):

| Obraz pomocniczy | PSNR dB | SSIM | ΔE2000 średnie | ΔE2000 p95 | FLIP LDR średnie | FLIP HDR średnie | Bias jasności |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| **Szum wzorca**: surowy 4096 vs odszumiony 4096 | 31,6 | 0,78 | 2,1 | 5,5 | 0,04 | 0,11 | 1,00 |
| **Szum 512 próbek** (tylko `renderCam`): surowy 512 vs odszumiony 4096 | 23,2 | 0,51 | 5,0 | 14,1 | 0,10 | 0,25 | 1,00 |
| **Stały ambient**: światło bezpośrednie + oświetlenie pośrednie zastąpione średnią z obrazu | 15,5 | 0,67 | 11,6 | 26,3 | 0,44 | 0,62 | 0,76 |
| **Bez GI**: tylko światło bezpośrednie i emisja | 11,4 | 0,39 | 17,0 | 66,8 | 0,45 | 0,82 | 0,40 |

(średnie z trzech kamer, `lights-only`, referencja odszumiona; wariant `ibl` daje liczby w granicach ±0,02 SSIM i ±0,2 ΔE). „Stały ambient” przybliża dawny ambient viewera: oddaje część energii (bias 0,76 zamiast 0,40), a mimo to jest daleko, bo **brakuje struktury**, nie tylko jasności (ciemne zakamarki, jaśniejsze strony zwrócone ku oknom). To orientacyjny punkt startu dla renderera bez GI, AO i cieni z lamp; rzeczywisty pomiar pathon może wypaść inaczej.

Gdzie błąd siedzi (kamera `renderCam`, `lights-only`, referencja odszumiona; bias = stosunek luminancji obrazu pomocniczego do wzorca w masce):

| Maska | Udział obrazu | Bias „bez GI” | Bias „stały ambient” | ΔE2000 „stały ambient” |
| --- | ---: | ---: | ---: | ---: |
| tablica | 4,9% | 0,89 | 1,11 | 2,7 |
| podłoga | 6,9% | 0,75 | 1,29 | 4,2 |
| okna (ramy, żaluzje) | 5,9% | 0,12 | 0,26 | 19,7 |
| ściany i sufit | 30,2% | 0,61 | 0,99 | 13,8 |
| drewno mebli | 24,8% | 0,55 | 1,53 | 8,9 |
| metal | 4,7% | 0,60 | 1,54 | 11,8 |
| blaty plastikowe | 7,3% | 0,69 | 1,00 | 3,4 |
| oświetlone bezpośrednio (połowa pikseli) | 45,0% | 0,57 | 0,78 | 7,3 |
| zacienione (połowa pikseli) | 55,0% | 0,21 | 0,82 | 14,1 |
| emisja (lampy) | 1,3% | 0,99 | 1,01 | 1,0 |

Stały ambient przejaskrawia drewno i metal (1,5×, bo wnętrza ławek są w rzeczywistości zaciemnione: tu trzeba AO, K04) i wciąż niedoświetla powierzchnie oświetlone bezpośrednio (0,78, bo nie ma odbić od otoczenia, tu trzeba GI). Okna i ściany są największym problemem koloru.

### Światła obszarowe: dwa wzorce

Wzorzec `real` (domyślny) zastępuje dwa półsferyczne spoty z `.glb` prostokątnymi światłami obszarowymi z `extras.pathon_area` (lampa za oknem 0,75 × 8,3 m, 1963 W; podświetlenie tablicy 2,43 × 0,11 m, 23,6 W). Pathon przed K05 rysuje spoty, które mają połowę natężenia na osi, ale świecą jednakowo pod dużymi kątami (prostokąt Lamberta słabnie z kosinusem, a lampa za oknem oświetla pokój głównie ukośnie), więc spoty wychodzą jaśniejsze; same światła obszarowe zmieniają obraz:

| Kamera | Bias jasności spot / obszarowe | ΔE2000 średnie / p95 | SSIM | FLIP LDR średnie | PSNR dB |
| --- | ---: | ---: | ---: | ---: | ---: |
| `renderCam` | 1,06 | 4,1 / 15,7 | 0,92 | 0,19 | 21,9 |
| `pathon_wide` | 1,33 | 4,2 / 16,6 | 0,91 | 0,19 | 21,6 |
| `pathon_reverse` | 1,78 | 3,6 / 13,1 | 0,94 | 0,17 | 22,4 |

(obie strony `lights-only` odszumione). Sam przybliżony kształt lampy za oknem to więc **od 6 do 78% jasności**, a ΔE2000 i SSIM wyglądają przy tym łagodnie, bo tonemap PBR Neutral spłaszcza jasne partie: **`bias_total` jest warunkiem koniecznym i nie wolno bramkować wyłącznie ΔE czy SSIM**. Do bramkowania technik w okresie przed K05 używaj `reference-spot/` (ta sama struktura plików, 1024 próbki, odszumione i surowe, oba warianty światła); po K05 przełącz się na `reference/`. W obu wzorcach mierzy się to samo: kamery, materiały, ekspozycja, GI.

## Progi: zielone, żółte, czerwone (propozycja)

Zaproponowane po obejrzeniu pierwszych wyników, **kotwice to obrazy pomocnicze z tabeli wyżej, a nie wyniki pathon** (pomiaru pathon nie ma). Zasada: zielone = obraz nie do odróżnienia od ścieżkowego renderu w ok. 500 próbek, żółte = widoczne różnice, ale lepsze niż stały ambient, czerwone = nie lepsze niż stały ambient. Średnie z trzech kamer na wzorcu `reference-spot` albo `reference`, referencja odszumiona.

| Metryka | Zielone | Żółte | Czerwone | Kotwice |
| --- | --- | --- | --- | --- |
| `bias_total` | 0,90 do 1,10 | 0,76 do 0,90 albo 1,10 do 1,30 | poniżej 0,76 albo powyżej 1,30 | stały ambient 0,76, bez GI 0,40 |
| ΔE2000 średnie | do 6 | 6 do 11 | powyżej 11 | szum 512 próbek 5,0; stały ambient 11,6 |
| ΔE2000 p95 | do 15 | 15 do 26 | powyżej 26 | szum 512 próbek 14,1; stały ambient 26,3 |
| SSIM | od 0,80 | 0,67 do 0,80 | poniżej 0,67 | stały ambient 0,67 |
| FLIP LDR średnie | do 0,15 | 0,15 do 0,40 | powyżej 0,40 | szum 512 próbek 0,10; stały ambient 0,44 |
| PSNR dB | od 23 | 16 do 23 | poniżej 16 | szum 512 próbek 23,2; stały ambient 15,5 |
| `per_mask.*.bias_linear` | 0,85 do 1,18 | 0,70 do 0,85 albo 1,18 do 1,40 | poza | |

Uwagi: (1) `bias_total` czytaj razem z ΔE (patrz wyżej: spot zamiast światła obszarowego daje łagodne ΔE przy biasie 1,78); to zbiór progów dla **całego obrazu**; maski czytamy względnie (poprawa względem poprzedniego pomiaru), bo w cieniu i w oknach nawet dobry render ma duże błędy. (2) Progi bezwzględne należy zrewidować po pierwszym pomiarze pathon; do tego czasu obowiązuje reguła z [filament-reference.md](filament-reference.md#użycie-w-pracy-nad-techniką): żadna metryka nie pogarsza się o więcej niż 5% względnie bez uzasadnienia, a PR techniki dołącza tabelę przed i po. (3) FLIP i SSIM zależą od szumu referencji; zawsze używaj `*_denoised.exr` i tego samego pliku przed i po.

## Znane różnice, których nie usuniemy

| Różnica | Skutek | Kiedy znika |
| --- | --- | --- |
| **Brak GI w pathon** | 55 do 64% luminancji, patrz wyżej | K12 (sondy), K13 (SSGI); częściowo K04 (AO) |
| **Światła obszarowe jako spoty** | jasność o 6 do 78%, ΔE około 4 | K05; do tego czasu `reference-spot/` |
| **Okna bez portalu światła dziennego** | otwory w ścianie, w `lights-only` czarne; w obu wzorcach identycznie | nie znika (pathon i wzorzec tak samo; portal tylko w wzorcu A) |
| **Materiały po eksporcie Cycles → Principled** | straty z [scenes.md](scenes.md): bump (37), korekcje koloru (21), mieszanie tekstur (12), atrybuty obiektu (16), shadery proceduralne (12), roughness z tekstury (9) | nie znika; oba renderery dostają te same przybliżone materiały, więc **w tym porównaniu nie liczy się jako błąd pathon**, ale pozostaje różnicą względem wzorca A (`classroom.blend`) |
| **Model materiału** | Principled BSDF Cyclesa (wielokrotne rozpraszanie GGX, inny sposób łączenia diffuse i specular) a materiał standardowy pathon; transmisja: refrakcja a blend z alfą | częściowo K02; różnica modeli zostaje |
| **Cienie lamp punktowych** | Cycles: promień 0, ostre; pathon: PCF o skończonej szerokości (K03) | K03/K10 |
| **Filtrowanie tekstur** | Cycles bez mipmap i anizotropii (filtr liniowy), pathon z mipmapami i anizotropią | nie znika; widać na dalekich, skośnych teksturach |
| **Antyaliasing** | Cycles: filtr box 1 px z 4096 próbek; pathon: `--supersample 2` albo TAA | K06 |
| **Kaustyki i objętość** | wyłączone w Cyclesie; w eksporcie pominięto też objętość i kurz | — |
| **Kamera `renderCam`** | w `.glb` leży o 2 mm (x) i 7 mm (z) od `classroom.scene.json`; wzorzec używa `.glb` jak loader pathon | — |
| **Odszumianie wzorca** | OpenImageDenoise może rozmyć drobny detal (napisy na tablicy; nie sprawdzano); `bias_total` surowego i odszumionego różni się o ±0,7% | nie znika; w razie wątpliwości porównaj też z surowym |
| **Obcinanie indirect** | wyłączone, więc surowy obraz ma świetliki | — |

## Zastrzeżenia

- **Nie ma jeszcze żadnego pomiaru pathon.** Kontrakt strony pathon (plik `.npy` z liniowym `hdr`, ekspozycja 1/683) jest spisany, ale przykład, który go zapisuje, nie istnieje; komparator sprawdzono na obrazach pomocniczych i na wzorcu względem siebie. Jednostki światła (K = 1) zmierzono w Blenderze, ale nie po stronie pathon.
- Progi są propozycją zakotwiczoną w obrazach pomocniczych.
- Skrypt Blendera i komparator uruchomiono tylko na Windows (Blender 5.2.2, HIP RT, Python 3.11, numpy 2.4, Pillow 12). Blender 4.5 nie ma w tej instalacji `blender.exe`; API 5.x (media_type) może nie działać na 4.x.
- Licencja `classroom` i `Garage.hdr` nadal do potwierdzenia ([scenes.md](scenes.md)); PNG w `docs/scenes/reference/` są jej wynikami, więc obowiązuje to samo zastrzeżenie.
- Wzorzec A (`classroom.blend` z oryginalnymi materiałami, portalami i światłami) **nie jest zrenderowany**: skrypt obsługuje tylko `.glb`.
- Cryptomatte zastąpiono indeksem materiału (Material Index); maski po obiekcie nie ma.
- Nie ma metryki RMSE względnego w paśmie Indirect Diffuse (wymaga osobnego przebiegu po stronie pathon).
- FLIP: pakiet `flip-evaluator` jest w PyPI na licencji BSD-3-Clause (LICENSE repozytorium NVlabs/flip: „BSD 3-Clause”); używamy go jako zewnętrznego narzędzia dev w venv poza repozytorium, bez zależności Cargo. Zainstalowano go w venv w katalogu roboczym tego zadania (wersja 1.7), nie globalnie.

