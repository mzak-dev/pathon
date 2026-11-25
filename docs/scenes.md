# Sceny testowe

Duże sceny leżą poza gitem (`test-assets/` jest w `.gitignore`). W repozytorium są tylko skrypty eksportu i ten opis. Loader glTF żyje wyłącznie w `examples/` (M4-10), rdzeń i paczki go nie znają.

## Classroom

Wnętrze szkolnej klasy z fizycznie sensownymi materiałami i wieloma źródłami światła; w Blenderze powstała do testów Cyclesa (światło odbite, wiele świateł). Autor: Christophe Seux. Plik `classroom.blend` z 13.06.2019, z demo plików Blendera.

- **Źródło:** `https://download.blender.org/demo/test/classroom.zip` (skrót: lista „Demo Files” na blender.org).
- **Licencja:** CC0 według strony demo plików Blendera, potwierdzone wtórnie przez Wikimedia Commons (render sceny oznaczony CC0 1.0). **Niepewność:** `ReadMe.txt` w archiwum nie zawiera żadnej licencji, a samą stronę blender.org udało się sprawdzić tylko pośrednio (bezpośrednie pobranie zwróciło 403). Przed upublicznieniem wyników z tą sceną (np. golden images w repo, demo w Chrome) warto potwierdzić licencję ręcznie na stronie.
- **`Garage.hdr`:** `assets/__ENV/Garage/Garage.hdr` jest w tym samym archiwum, ale jego pochodzenia i licencji nie znamy. Traktuj go jako lokalny asset testowy: nie commituj go i nie publikuj wyników z nim bez sprawdzenia.
- **Zawartość `.blend`:** scena `_mainScene` (wnętrze, Cycles), `_dustParticules` i `_volumeLight` (efekty dla Blender Internal / EEVEE). Eksportujemy tylko `_mainScene`.

### Eksport

Sprawdzone na Blenderze 5.2.2 LTS (w katalogu „Blender 4.5” na tej maszynie brakuje `blender.exe`; 4.x ma ten sam eksporter glTF, ale skryptu na nim nie uruchamiano). Archiwum rozpakuj do `test-assets/classroom-src/classroom/` (folder `assets/` i `textures/` obok `classroom.blend`, bo scena linkuje do nich bibliotekami).

```sh
blender -b test-assets/classroom-src/classroom/classroom.blend \
  --python tools/scenes/export_classroom.py -- test-assets/classroom
```

| Opcja | Domyślnie | Znaczenie |
| --- | --- | --- |
| `--variant full\|lite\|all` | `all` | które pliki wygenerować |
| `--max-texture-size N` | 2048 | limit dłuższego boku tekstur w wersji pełnej |
| `--lite-texture-size N` | 1024 | to samo dla `classroom-lite` |
| `--env-hdr PLIK` | `assets/__ENV/Garage/Garage.hdr` obok `.blend` | HDR kopiowany do `env/` |
| `--no-extra-cameras` | wyłączone | bez kamer pomocniczych `pathon_*` |

Skrypt niczego nie zapisuje do `.blend` ani nie zmienia oryginalnych tekstur: wszystko dzieje się w pamięci sesji Blendera. Całość (wczytanie `.blend` ok. 13 s, eksport obu wariantów ok. 20 s) trwa pół minuty. Jeśli zapis `.glb` zawiedzie z `Invalid argument` (zdarzyło się raz, prawdopodobnie skaner antywirusowy trzymający poprzedni plik), uruchom ponownie.

### Wynik

W `test-assets/classroom/`:

| Plik | Zawartość |
| --- | --- |
| `classroom.glb` | pełna scena, tekstury do 2048 px, PNG/JPEG jak w źródle |
| `classroom-lite.glb` | wariant szybki, tekstury do 1024 px, wszystko co się da jako JPEG |
| `classroom.scene.json`, `classroom-lite.scene.json` | manifest: kamery, światła z przeliczeniem jednostek, raport konwersji materiałów, statystyki |
| `env/Garage.hdr` | równoprostokątny Radiance HDR (do IBL, M4-06) |

| | pełna | lite |
| --- | ---: | ---: |
| rozmiar | 36,4 MB | 17,9 MB |
| węzły / meshe / prymitywy | 956 / 347 / 356 | 915 / 323 / 329 |
| trójkąty | 443 594 | 372 258 |
| materiały / tekstury / obrazy | 77 / 45 / 28 | 68 / 42 / 25 |
| światła (definicje / węzły) | 5 / 10 | 5 / 10 |
| kamery | 3 | 3 |
| tekstury przeskalowane | 2 | 9 |

Lite pomija drobiazgi: ołówki, długopisy, gumki (`pencil`, `plasticGoldPen`, `rotring`), duże pudło (`largeBox`) i 13 kulek papieru (`crinkledPaper`); dla tekstur bez kanału alfa używa JPEG q85.

Meshe z modyfikatorami (Subsurf, Array, Mirror, Hook, Lattice, Armature) są zastosowane i wypieczone w aktualnej pozie (żaluzje), bez szkieletów i animacji. Instancje grup (`ceilingLamp`, `schoolDesk` i inne) są rozwinięte do zwykłych węzłów; `EXT_mesh_gpu_instancing` nie jest używane, bo eksporter obsługuje je tylko dla dzieci jednego empty, więc geometria instancji jest duplikowana (stąd 347 meshy przy 167 unikalnych nazwach).

### Jednostki i światła

Układ glTF: prawoskrętny, Y w górę, metry (zgodnie z konwencją pathon). Światła w `KHR_lights_punctual`, konwersja jak w eksporterze Blendera (tryb `SPEC`, stała 683 lm/W):

| Typ w Blenderze | glTF | Wzór |
| --- | --- | --- |
| Sun (W/m²) | `directional`, lux | `lux = W/m² · 683` |
| Point i Spot (W) | `point` / `spot`, kandela | `cd = W / (4π) · 683` |
| Area (W) | `spot` z kątem zewnętrznym ok. 90°, kandela | `cd = W / (2π) · 683` (strumień zachowany w półkuli); rozmiar i kształt w `extras.pathon_area` |

W Blenderze moc światła to iloczyn pola `Strength` lampy i `Strength` węzła Emission (albo Light Falloff). Eksporter by to przekłamywał, więc skrypt przelicza efektywną moc sam i czyści drzewo węzłów lampy. Kolor Blackbody (5800 K słońce, 5000 K lampy sufitowe) jest liczony z krzywej Plancka i normalizowany do maksimum 1. Wartości z tej sceny: słońce 15 W/m² = 10 245 lx, lampa korytarza 60 W = 3261 cd, lampy sufitowe 20 W = 1087 cd (6 sztuk), podświetlenie tablicy 2561 cd, wypełnienie za oknem (`exterior_fillLight`, 1963 W, prostokąt 0,75×8,3 m) 213 437 cd.

Dwie uwagi o skali. Po pierwsze, „waty” Blendera nie są radiometrycznymi watami, a konwencja `/(4π)·683` pochodzi z eksportera: dostajemy wartości spójne z innymi narzędziami glTF, ale jasność w Cyclesie bywa o czynnik rzędu π niższa. Po drugie, Blender renderuje z ekspozycją 0 i transformacją Standard, więc żeby odtworzyć jego wygląd, slot `exposure` powinien wynosić około 1/683. Świat w oryginale jest czarny (siła 0), więc sam HDR do IBL jest dodatkiem pathon, nie częścią oryginalnego oświetlenia.

### Kamery

W `<nazwa>.scene.json`, pole `cameras` (pozycja w układzie glTF i Blendera, `yfov`, `znear`, `zfar`, proporcje 16:9, flaga `main`):

- `renderCam`: główna, 25 mm na sensorze 32 mm (yfov 0,691 rad), pozycja glTF (2,58, 1,09, 4,47);
- `pathon_wide`: ta sama pozycja, 14 mm;
- `pathon_reverse`: od strony tablicy w stronę `renderCam`, 20 mm.

Kamery `pathon_*` dodaje skrypt (nie ma ich w `.blend`), by test miał więcej niż jeden widok.

### Materiały: Principled BSDF i straty

Oryginał jest klasycznym materiałem Cyclesa (Diffuse + Glossy w Mix Shader, Glass, Emission), a Principled BSDF występuje w dwóch materiałach nieużywanych w kadrze. Eksporter glTF rozumie tylko Principled, więc skrypt buduje dla każdego materiału równoważny Principled BSDF i dopiero ten eksportuje:

- Diffuse: base color (stała albo tekstura), metallic 0, roughness 0,9.
- Glossy samodzielny (metale): metallic 1, base color z tekstury, roughness z wartości.
- Mix Diffuse + Glossy z małym udziałem glossy (typowo 1 do 10%): dielektryk, roughness interpolowana od 0,9 do roughness glossy; metallic dopiero od udziału 0,5.
- Glass: `KHR_materials_transmission` + `KHR_materials_ior`, roughness z oryginału.
- Emission: emissive i `KHR_materials_emissive_strength` (lampy sufitowe 2,0, kolor Blackbody).
- Transparent w miksie: alfa stała albo z kanału alfa tekstury (tryb `BLEND`; siatka kosza, kulki papieru).
- Tekstury ambient occlusion (`*_AO`) trafiają do `occlusionTexture` (przez grupę `glTF Material Output`).
- Mapping dołączony do UV przechodzi jako `KHR_texture_transform`.

Zdecydowanie nie jest to wierne odtworzenie Cyclesa. Znane straty (liczby z manifestu, `material_loss_summary`, to liczba wystąpień w 81 przekonwertowanych materiałach, z czego 77 trafia do pliku):

| Strata | Wystąpień | Jak obejść |
| --- | ---: | --- |
| Bump (`bump_or_normal_ignored`) | 37 | wypiec normal mapę z Cyclesa albo dorobić `normalTexture` z wysokości |
| Displacement | 7 | j.w.; geometria tych materiałów jest płaska |
| Węzły korekcji koloru (HSV, krzywe RGB, Gamma, Invert, Brightness/Contrast) | 21 | tekstura bez korekcji; wypiec base color (Cycles `DIFFUSE` bez światła) |
| Mieszanie dwóch tekstur w base color | 12 | wybierana tekstura o większej wadze (np. tapeta: farba + tynk) |
| Atrybuty obiektu jako kolor (`Attribute`, np. skóra foteli) | 16 | stały szary / kolor z tekstury; wypiec albo ustaw kolory ręcznie |
| Shadery proceduralne (Noise, Sky, Wave) | 12 | stała zastępcza; wypiec do tekstury |
| Roughness z tekstury | 9 | stała 0,4; wypiec roughness |
| Mix Shader sterowany teksturą (maska) | 4 | alfa z tekstury albo wybór dominującego shadera |

Dodatkowo:

- **Okna dzienne:** obiekty z materiałem `dayLight_portal` (Emission z węzła Sky Texture za Transparent, czyli portal światła dziennego Cyclesa) są pomijane, inaczej zasłaniałyby okna. W pathon światło dzienne to `sun` + opcjonalnie IBL.
- **Zestawy poza `_mainScene`:** `_dustParticules` (cząsteczki kurzu) i `_volumeLight` (stożki objętościowe w oknach i przy tablicy, w tym 5 spotów) nie są eksportowane; pathon nie ma volumetryki.
- **Światła obszarowe:** glTF ich nie zna, patrz tabela wyżej; kształt jest tylko w `extras`.
- **Żaluzje:** eksportowane w pozie z pliku, bez szkieletów (`export_skins=False`); 8 szkieletów i haki są zastosowane do geometrii.
- **Dwustronność:** Blender domyślnie nie odrzuca tylnych ścianek, więc wszystkie 77 materiałów ma `doubleSided`. Dla wydajności można to później włączyć selektywnie.
- **Siatki bez materiału** (6 prymitywów: pomocnicze linie) i puste siatki pomocnicze są pomijane przez eksporter z ostrzeżeniem.
- **Kolory wierzchołków** i animacje: nieeksportowane.
- **Tekstury:** przeskalowane są tylko obrazy większe niż limit (w oryginale mają głównie 512 do 1024 px; tylko dwa przekraczają 2048, więc pełna wersja prawie nie traci jakości). Format pełnej wersji: PNG/JPEG jak w źródle, więc część PNG bez alfy jest niepotrzebnie duża (stąd 24 MB obrazów z 36 MB).

### Weryfikacja

Sprawdzone przy tworzeniu (22.11.2026, Blender 5.2.2):

- Walidator Khronos [`gltf-validator`](https://github.com/KhronosGroup/glTF-Validator) 2.0.0-dev.3.10 (npm, w izolowanym katalogu): `classroom.glb` i `classroom-lite.glb` bez błędów i ostrzeżeń (50 informacji `UNUSED_OBJECT`). Pierwsza wersja miała błąd `outerConeAngle > π/2` dla świateł obszarowych; naprawiony w skrypcie.
- Ponowny import `.glb` do Blendera (`import_scene.gltf`): 956 obiektów (836 meshów, 107 empty, 10 świateł, 3 kamery), 28 obrazów; render Workbench z `renderCam` pokazuje poprawną klasę (tablica z napisami, tablice korkowe, mapa, drzwi, ławki z teksturami, podłoga).
- Własne sprawdzenia JSON-a glb: brak prymitywów z teksturą bez `TEXCOORD_0`, 77 materiałów, 2 z `BLEND`.

Render w pathon: patrz [Render w pathon](#render-w-pathon) niżej.

Walidator uruchomisz tak:

```sh
npm install gltf-validator && node -e "require('gltf-validator').validateBytes(new Uint8Array(require('fs').readFileSync('test-assets/classroom/classroom.glb'))).then(r=>console.log(r.issues))"
```

### Użycie w testach

Konwencja M4-10: zmienna środowiskowa `PATHON_TEST_SCENE` wskazuje na plik `.glb`, który przykłady i testy wczytują; bez niej testy sceny się pomijają (z komunikatem). Obsługują ją `gltf_viewer` (domyślny plik) i test `the_test_scene_loads_when_it_is_available` w `examples/src/gltf_loader.rs`. Wymuszenia w CI (jak `PATHON_REQUIRE_WINDOW` przy oknach) nie ma: sceny nie ma w repozytorium.

```sh
PATHON_TEST_SCENE=test-assets/classroom/classroom-lite.glb cargo run --release -p pathon-examples --bin gltf_viewer
PATHON_TEST_SCENE=test-assets/classroom/classroom-lite.glb cargo test -p pathon-examples --lib
```

Do codziennych testów i Chrome używaj `classroom-lite.glb` (17,9 MB), do pomiarów wydajności `classroom.glb`. Do IBL wskaż `test-assets/classroom/env/Garage.hdr` (M4-06). Główna kamera i światła są w `*.scene.json`; loader bierze z niego tylko flagę `main` kamery (nazwa obiektu = nazwa węzła glTF), reszta idzie z samego `.glb`.

## Render w pathon

`cargo run --release -p pathon-examples --bin gltf_viewer` (opis, sterowanie i tryb `--screenshot` w [przykładach](examples.md#viewer-sceny-testowej-gltf_viewer)). Sprawdzone 22.11.2026 na Windows, Vulkan, AMD Radeon RX 9070 XT, w obu wariantach sceny: wczytanie `.glb` ok. 0,1-0,5 s (obrazy dekodowane równolegle), wgranie na GPU poniżej sekundy, w oknie 1280×720 ok. 70 klatek na sekundę przy włączonym vsync (`classroom.glb`: 862 instancje, 606 tys. trójkątów po rozwinięciu instancji, 78 materiałów, 10 świateł).

Zrzuty referencyjne (960×540, ekspozycja domyślna viewera, `--supersample 2`) leżą w `docs/scenes/`:

| Plik | Co | Polecenie (`--size 960x540 --supersample 2`) |
| --- | --- | --- |
| `classroom-lite-renderCam.png` | kamera główna, wariant lite | `classroom-lite.glb --camera 2` |
| `classroom-lite-wide.png` | `pathon_wide` (14 mm) | `classroom-lite.glb --camera 1` |
| `classroom-lite-reverse.png` | `pathon_reverse`, od strony tablicy | `classroom-lite.glb --camera 0` |
| `classroom-renderCam.png` | kamera główna, wariant pełny | `classroom.glb --camera 2` |

Numer kamery to kolejność z pliku (log viewera ją wypisuje; w klasie: 0 `pathon_reverse`, 1 `pathon_wide`, 2 `renderCam`). Pliki są wygenerowane lokalnym wariantem sceny, której licencji nie potwierdzono do końca (patrz wyżej): zanim obrazy wyjdą poza repozytorium, trzeba to zrobić.

Co widać i co nie jest wierne (viewer to przejściowy zamiennik pbr, cieni, IBL i postu):

- **Zgadza się:** tablica z napisami, mapa, tablice korkowe, drzwi, ławki i krzesła z drewnem i metalem, żaluzje, kosz z siatki (blend z maski alfa) z kulkami papieru, lampy sufitowe jako emisja, plamy słońca na podłodze przez żaluzje (cień kierunkowy z mapy 4096²). Normalne, winding i odwrócenie Y są poprawne; lustrzane instancje (ujemny wyznacznik) nie mają odwróconych normalnych.
- **Jasność:** ekspozycja 1/683 z tego opisu jest u Blendera przy transformacji Standard; z ACES (przybliżenie Narkowicza w shaderze viewera) obraz jest za ciemny, więc viewer startuje o jeden stopień wyżej (2/683). Klawisze `+` / `-` zmieniają ekspozycję, `--ev` w trybie zrzutu.
- **Oświetlenie:** pętla po wszystkich 10 światłach bez cieni (poza słońcem) plus ambient półsferyczny 400 / 150 lx w miejsce IBL. Wypełnienie za oknem (213 437 cd) świeci też na powierzchnie, które w rzeczywistości zasłania ściana, a światła sufitowe nie rzucają cieni, więc kontakty (spód ławek) są jaśniejsze niż w Cyclesie.
- **Okna:** światło dzienne to słońce, a tło okien to stały kolor nieba (brak portalu i IBL).
- **Szkło** (`frostedGlass`, szkło zegara): `KHR_materials_transmission` jest rysowane jako blend z alfą `1 - 0,9 * transmission`, bez załamania.
- **Materiały:** straty z tabeli wyżej (bump, korekcje koloru, atrybuty obiektu) są widoczne jako płaskie albo jednolite powierzchnie (np. skóra foteli). `KHR_texture_transform` dla normalnej i okluzji bierze transformację base color, bo crate `gltf` nie wystawia jej dla tych dwóch map.
- **Alias:** okno nie ma MSAA (zrzuty wygładza `--supersample`).

## Render referencyjny w Cyclesie

Wzorzec do porównań obrazu (ten sam `.glb` odtworzony w Blenderze, trzy kamery, metryki, AOV): [porownanie-obrazu.md](porownanie-obrazu.md); skrypty `tools/scenes/render_reference.py` i `tools/scenes/compare.py`. Wyniki leżą w `test-assets/classroom/reference/` (poza gitem), małe PNG w `docs/scenes/reference/`.
