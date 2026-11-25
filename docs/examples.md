# Przykłady

Crate `examples/` gra rolę hosta, więc wolno mu używać wgpu i winit wprost (jak własny silnik).

| Przykład | Task | Tryb | Co pokazuje |
| --- | --- | --- | --- |
| `clear` | M0-10 | C | runner: okno na wątku głównym, renderer bez paczek na wątku renderu |
| `examples/web` | M0-06 | C | `clear` w przeglądarce (WebGPU), przez runner bez wątków ([ADR-0014](adr/0014-web-przez-winit-i-runner-bez-watkow.md)) |
| `external_window` | M0-11 | C | host ma okno i pętlę (surowy winit), woła `render_frame` na swoim wątku — bez `pathon-platform` |
| `multi_view` | M2-17 | C | host ma dwa okna (surowy winit) i cel offscreen 256×256, jeden renderer, trzy kamery; co 120 klatek odczyt piksela offscreen do logu |
| `vertical_slice` | M2-18 | C | sześcian unlit przez cały stos w dwóch oknach; `--devtools` udostępnia klatkę dla [devtools](devtools.md) |
| `mesh_pool` | M4-01 | — | trzy meshe z jednej puli mega-buforów rysowane offscreen (wynik jako tekst), potem usunięcie jednego i reuse jego zakresu ([pathon-mesh](crates/pathon-mesh.md)); bez rendera i okna, na samym `Device` |
| `profile` | M2-12 | C | wycinek pionowy offscreen pod okiem `pathon-profile`: tabela czasów GPU passów na stderr; `--features tracy` / `puffin` strumieniuje do profilera ([pathon-profile](crates/pathon-profile.md)) |
| `scene_extract` | M4-02 | — | host z własnym `Vec` obiektów kopiuje je do render worlda (`Scene::extract`): spawn, ruch części obiektów, despawn; log pokazuje listę zmian do wgrania, `previous_model`, mapowanie `MeshId` ↔ `MeshHandle` i odrzucony stary uchwyt ([pathon-scene](crates/pathon-scene.md)); bez GPU i okna |
| `texture_load` | M4-13 | — | tekstura sRGB przez `TextureLoader`: wybór formatu według `Caps` i mipmapy w compute; szachownica pokazuje uśrednianie w świetle liniowym (każdy mip 188, nie 128), wynik jako log ([pathon-texture](crates/pathon-texture.md)); bez rendera i okna, na samym `Device` |
| `material_sphere` | M4-03 | — | siatka 5 × 3 kul z materiałem standardowym (dielektryk, metal, lakier z clearcoatem; chropowatość rośnie w kolumnach), jedno światło kierunkowe i stały ambient zamiast IBL; wynik do pliku PNG (argument, domyślnie `material_sphere.png`), `cargo run -p pathon-examples --bin material_sphere` ([pathon-material](crates/pathon-material.md)); bez rendera i okna, na samym `Device` |
| `hdr_output` | M4-16 | C | raport, jakie przestrzenie barw oferuje powierzchnia okna i co wyświetla (`PATHON_COLOR_SPACE`, `PATHON_BACKEND`); bez tonemappingu, tylko możliwości ([ADR-0034](adr/0034-wyjscie-hdr.md)) |
| `post_chain` | M4-09 | — | syntetyczna klatka HDR (rampa 16 stopni, kolorowe paski, dwie jasne plamki) przez `PostPack`: auto-ekspozycja adaptuje się przez 90 klatek, bloom i tonemapping (`aces` albo `neutral`) do PNG, `cargo run -p pathon-examples --bin post_chain -- aces wynik.png` ([pathon-post](crates/pathon-post.md)); offscreen, bez okna |
| `bindless_scene` | M4-04 | — | podłoga, dwie kule i sześcian z trzema materiałami i sześcioma teksturami; tekstury idą przez `binding_array` (urządzenie z tablicami) albo bind group per materiał (Web); `--bindless` i `--per-material` wymuszają ścieżkę (oba obrazy są identyczne), argument to plik PNG (domyślnie `bindless_scene.png`), `cargo run -p pathon-examples --bin bindless_scene` ([pathon-bindless](crates/pathon-bindless.md)); bez rendera i okna, na samym `Device` |
| `pbr_scene` | M4-05 | — | podłoga, kule ze złota i czerwonego plastiku, słońce, dwa światła punktowe i reflektor, środowisko z syntetycznego nieba: `PbrPack` + `IblPack` + `PostPack` na offscreenie, wynik do PNG, `cargo run -p pathon-examples --bin pbr_scene -- wynik.png` ([pathon-pbr](crates/pathon-pbr.md)); siatki, tablica materiałów i migawka sceny idą do `Shared` |
| `ibl_spheres` | M4-06 | — | siatka 5 × 3 kul (dielektryk, złoto, srebro; chropowatość rośnie w kolumnach) oświetlona wyłącznie środowiskiem: niebo syntetyczne z słońcem albo plik Radiance `.hdr` (drugi argument); `IblBuilder` liczy prefiltr, irradiancję i LUT BRDF w compute raz, shader woła `ibl_evaluate` obok `load_surface`; wynik do PNG, `cargo run -p pathon-examples --bin ibl_spheres -- wynik.png [plik.hdr]` ([pathon-ibl](crates/pathon-ibl.md)); bez rendera i okna, na samym `Device` |
| `skin_arm` | M4-12 | — | ramię z dwoma kośćmi deformowane przez `SkinPack`: cztery klatki z rosnącym kątem łokcia (log: pozycja nadgarstka i wektor ruchu = poza bieżąca minus poprzednia, z drugiego banku `DeformedPositions`), potem trzy ramiona o różnych kątach narysowane z buforów wynikowych (normalne jako kolor) do PNG, `cargo run -p pathon-examples --bin skin_arm -- wynik.png` ([pathon-skin](crates/pathon-skin.md)); renderer z jednym offscreenem, bez okna |
| `gltf_viewer` | M4-10 | C | scena testowa glTF (classroom z [scenes.md](scenes.md)) w oknie z kamerą swobodną albo jedna klatka offscreen do PNG (`--screenshot`); loader glTF, wgranie do `pathon-mesh` / `-texture` / `-material` / `-scene` i własna paczka rysująca (cień słońca, forward ze światłami punktowymi, tonemapping ACES); opis niżej |
| `shadow_scene` | M4-07 | — | podłoga, stos sześcianów, kula, płaszczyzna skierowana do słońca i odległy sześcian oświetlone jednym światłem kierunkowym: `ShadowPack` renderuje do czterech kaskad, shader liczy `N·L × shadow_factor_directional`; w logu podział kaskad i rozmiar teksela, wynik do PNG, `cargo run -p pathon-examples --bin shadow_scene -- wynik.png [hard\|pcf3\|pcf5] [stable\|focused] [kaskady]` ([pathon-shadow](crates/pathon-shadow.md)); offscreen, bez okna; scena i shading te same co w teście golden |
| `host_foreign_device` | M2-19 | B | host tworzy urządzenie i swapchain; pathon przyjmuje urządzenie, renderuje do tekstury hosta, host kopiuje ją na ekran |

```sh
cargo run -p pathon-examples --bin vertical_slice
```

Przykłady okienkowe zatrzymują się same, gdy ustawisz `PATHON_EXIT_AFTER_FRAMES=N`: po N wyrenderowanych klatkach proces kończy się kodem 0, a log zawiera raport `Caps` (tak CI uruchamia `clear` bez człowieka). Dziś obsługują to `clear` i `hdr_output`.

```sh
PATHON_EXIT_AFTER_FRAMES=120 cargo run -p pathon-examples --bin clear
```

## Viewer sceny testowej (`gltf_viewer`)

Loader glTF 2.0 (`examples/src/gltf_loader*`, crate `gltf` plus `png` i `zune-jpeg`) żyje wyłącznie w `examples/`: rdzeń i paczki nie znają plików. Loader czyta `.glb` / `.gltf` (bufory i obrazy z `data:`, z pliku obok albo z fragmentu GLB) i nie dotyka GPU:

| Co | Dokąd trafia |
| --- | --- |
| prymitywy trójkątów | `CpuMesh` w układzie `VertexLayout::standard()`; brakujące normalne liczone z ścian, brakujące tangenty z gradientów UV (Lengyel, `w` = znak bitangenta, zgodnie z konwencją glTF: +V w dół) |
| obrazy PNG / JPEG | dekodowane równolegle (`std::thread::scope`) do RGBA8; dekodery tylko tu, `pathon-texture` bierze surowe teksele |
| materiały | `StandardMaterial`: metallic-roughness, `KHR_materials_{emissive_strength, ior, specular, transmission, volume, clearcoat}`, `KHR_texture_transform` (macierz 2×2 + offset na teksturę; normalna i okluzja pożyczają transformację base color, bo crate `gltf` nie wystawia jej dla tych typów) |
| węzły | spłaszczone do macierzy świata, jedna instancja na prymityw; światła i kamery bez skali |
| `KHR_lights_punctual` | `pathon_scene::Light`: lux dla kierunkowych, kandele × 4π → lumeny dla punktowych i reflektorów |
| `<plik>.scene.json` obok pliku | tylko flaga `main` kamery (domyślna kamera) |

`viewer::upload` wgrywa to przez crate'y pathon: meshe do `GeometryPool`, obrazy przez `TextureLoader` (widok sRGB dla kolorów, mipmapy w compute), materiały do `MaterialTable`, instancje, światła i kamery do `Scene::extract`. Paczka `ViewerPack` czyta migawkę sceny z `Shared` i rysuje trzema passami: cień słońca (mapa 4096², PCF 3×3, kierunkowe światło jest jedno), forward (`material::standard::{load_surface, evaluate}` dla każdego światła sceny, ambient półsferyczny w miejsce IBL, materiały przezroczyste i szkło sortowane od tyłu z blendem premultiplied) i tonemapping ACES do celu. Ekspozycja domyślnie 1/683 ([scenes.md](scenes.md)), emisja materiałów mnożona przez 683 (Blender podaje ją w watach, światła w lumenach).

**Tymczasowe elementy**, które zastępują kolejne zadania: tekstury przez bind group na materiał (hook `texture_source`, profil „Web”; M4-04 podstawi `binding_array` na Desktopie), ambient zamiast IBL (M4-06), jeden cień słońca zamiast kaskad (M4-07), tonemapping w shaderze viewera (M4-09), forward ze światłami w pętli (M4-05 daje klastry). Szkło (`transmission`) jest rysowane jako blend, bez załamania.

```sh
cargo run --release -p pathon-examples --bin gltf_viewer -- test-assets/classroom/classroom-lite.glb
PATHON_TEST_SCENE=test-assets/classroom/classroom.glb cargo run --release -p pathon-examples --bin gltf_viewer
```

Sterowanie: WASD albo strzałki (ruch), Q / E (dół, góra), Shift (szybko), przeciągnięcie myszą (rozglądanie), kółko (prędkość), 1–9 (kamery z pliku), `+` / `-` (ekspozycja o pół stopnia), `[` / `]` (ambient), T (cień słońca), I (stan do logu), Esc (koniec).

Tryb bez okna renderuje jedną klatkę offscreen (`Rgba8UnormSrgb`) i zapisuje PNG; tak powstały zrzuty w `docs/scenes/`:

```sh
cargo run --release -p pathon-examples --bin gltf_viewer -- test-assets/classroom/classroom-lite.glb   --screenshot docs/scenes/classroom-lite-0.png --camera 0 --size 960x540 --supersample 2
```

`--camera N` to numer z logu (kolejność z pliku; `renderCam` jest główną), `--ev` przesuwa ekspozycję o stopnie, `--no-shadows` wyłącza cień słońca, `--supersample N` renderuje N razy większy obraz i uśrednia go w świetle liniowym.

Testy loadera (`cargo test -p pathon-examples --lib`) budują scenę syntetyczną w pamięci (trójkąt, materiał z teksturą PNG w base64, światło, kamera) i liczą zasoby; `PATHON_TEST_SCENE=…/classroom-lite.glb` dokłada test prawdziwej sceny (bez zmiennej test się pomija z komunikatem).

## Przykład webowy

Wymaga `wasm-pack` i celu `wasm32-unknown-unknown`. Bez WebGPU strona pokazuje komunikat zamiast obrazu.

```sh
examples/web/build.sh        # albo examples/web/build.ps1
cd examples/web && python -m http.server 8000
# otwórz http://127.0.0.1:8000/index.html
```

`examples/web/verify.cjs` sprawdza zbudowany przykład w Chromium (adapter, klatki, kolor canvasu, `Caps` kontra `navigator.gpu`); opis i kody wyjścia w [weryfikacji platform](weryfikacja-platform.md).

Wyjście (`examples/web/pkg/`) nie jest w repozytorium. CI buduje przykład w release i wypisuje rozmiar `.wasm` (surowy i po gzip) w podsumowaniu zadania `wasm`.

## Test szwu hosta

`examples/tests/host_seam.rs` (feature `gpu-tests`) sprawdza generyczność rdzenia na prawdziwym GPU:

- **tryb B** — scena wyrenderowana do tekstury hosta na jego urządzeniu jest identyczna piksel w piksel z trybem C (własne urządzenie, cel offscreen);
- **tryb A** — `record_frame` + `interop::encode_into` w enkoderze hosta: pathon rysuje na obrazie hosta i go nie czyści.

```sh
cargo test -p pathon-examples --features gpu-tests
```

## Test zdarzeń okna

`examples/tests/window_events.rs` (feature `window-tests`, bez harnessu testowego, bo macOS wymaga wątku głównego) otwiera prawdziwe okno przez `WinitPlatform`, zmienia jego rozmiar z kodu i czeka na `PlatformEvent::Resized`. Bez wyświetlacza (Linux bez `DISPLAY` i `WAYLAND_DISPLAY`) test się pomija, chyba że ustawiono `PATHON_REQUIRE_WINDOW=1`. `PATHON_EXPECT_SCALE` (z `WINIT_X11_SCALE_FACTOR` na X11) dodatkowo sprawdza skalę okna.

```sh
cargo test -p pathon-examples --features window-tests --test window_events
```

## Test rekonfiguracji powierzchni

`examples/tests/surface_reconfigure.rs` (też `window-tests` i bez harnessu) renderuje klatki do prawdziwego okna i po nich rekonfiguruje powierzchnię: te same parametry, kilka zmian rozmiaru okna, kolejne przestrzenie barw. Każda klatka ma się wyrenderować (bez R0009). Pilnuje błędu DX12, w którym `ResizeBuffers` padał, bo widoki poprzedniej klatki jeszcze żyły ([ADR-0034](adr/0034-wyjscie-hdr.md)). `PATHON_BACKEND=dx12|vulkan|metal` wybiera backend; bez wyświetlacza lub adaptera test się pomija, chyba że ustawiono `PATHON_REQUIRE_GPU=1`.

```sh
PATHON_BACKEND=dx12 PATHON_REQUIRE_GPU=1 cargo test -p pathon-examples --features window-tests --test surface_reconfigure
```
