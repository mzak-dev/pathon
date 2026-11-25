# pathon-debug

Narzędzia diagnostyczne (M4-14): linie, gizmo, AABB, wireframe i podgląd slotów, narysowane na `Slot::OUTPUT` po tonemappingu. Decyzje: [ADR-0046](../adr/0046-pathon-debug-linie-wireframe-i-podglad-slotow.md). Zależy tylko od `pathon-api` (`kind = "pack"`), więc nie zna żadnej innej paczki: z producentami slotów łączy ją wyłącznie kontrakt z [docs/slots.md](../slots.md).

## Użycie

```rust,ignore
use pathon_debug::{DebugColor, DebugDraw, DebugPack, DebugSettings, PreviewSlot, SlotPreview};

let mut renderer = Renderer::builder()
    .with_pack(DebugPack::new())
    .build()?;

// między klatkami; `DebugDraw` i `DebugSettings` leżą w `Shared`
let shared = renderer.shared_mut();
let draw = shared.get_mut::<DebugDraw>().expect("debug pack");
draw.aabb(Vec3::splat(-1.0), Vec3::splat(1.0), LineStyle::new(DebugColor::YELLOW).thickness(2.0));
draw.axes(Mat4::IDENTITY, 1.0, 3.0);

shared.get_mut::<DebugSettings>().expect("debug pack").preview =
    Some(SlotPreview::corner(PreviewSlot::Normals, Corner::TopRight));
```

Przykład: `cargo run -p pathon-examples --bin debug_slots -- katalog_wynikowy` zapisuje do PNG podgląd każdego slotu bazowego, siatkę wszystkich naraz, wireframe i linie.

## `DebugDraw`

Tryb natychmiastowy, w pamięci, bez I/O, we współrzędnych świata. Aplikacja dodaje prymitywy między klatkami; paczka rysuje je w każdym widoku i wycofuje.

| Metoda | Co dodaje |
| --- | --- |
| `line`, `polyline` | odcinek, łamana (opcjonalnie zamknięta) |
| `aabb`, `cuboid` | 12 krawędzi pudełka osiowego / sześcianu `[-0.5, 0.5]³` przesuniętego macierzą |
| `circle`, `sphere` | okrąg w płaszczyźnie o normalnej `n` / trzy okręgi osiowe |
| `arrow`, `axes` | strzałka (trzon i cztery żagielki) / gizmo transformacji: X czerwony, Y zielony, Z niebieski |
| `frustum` | ostrosłup kamery od płaszczyzny bliskiej do `far` metrów (działa też dla nieskończonej płaszczyzny dalekiej) |
| `wireframe` | trójkąty siatki w świecie; rysowane przez ścieżkę wireframe |

Tekst jest poza zakresem (ADR-0046): gizmo i podglądy nie mają podpisów, a siatka podglądu ma kolejność z `SlotPreview::slots`.

`LineStyle` (z `DebugColor` przez `Into`): kolor (to, co ma być widać na ekranie, sRGB), `thickness` w **pikselach** (0.1…64), `depth_tested` (ukryta za `Slot::DEPTH`, gdy jakaś paczka go pisze) i `persistent(n)` — rysowana przez `n` klatek. Historia klatek to ta żywotność: linia odlicza klatkę dopiero, gdy klatka ją naprawdę pokazała, więc ponowne nagranie tej samej klatki (ADR-0022) i linia dodana po klatce niczego nie zjadają. Nieskończone współrzędne są pomijane, kolory i grubości naprawiane.

Limity (`DebugDraw::with_capacity`, domyślnie 262 144 linii i 65 536 trójkątów): nadmiar jest odrzucany i zgłaszany raz jako [U0002](../diag/U0002.md).

## Linie

`LinesFeature` (`Stage::Overlay`, priorytet 20) rysuje odcinki jako czworokąty w przestrzeni ekranu, po jednej instancji na odcinek (bufor wierzchołków z krokiem per instancja, więc działa na każdym profilu, także bez storage buffers w vertex shaderze). Fragment liczy odległość od odcinka: grubość jest w pikselach niezależnie od odległości i skali renderu, krawędź ma rampę 1 px, końce są zaokrąglone. Odcinek przecięty płaszczyzną bliską jest przycinany. Linia z `depth_tested` porównuje głębię fragmentu ze `Slot::DEPTH` (reverse-Z, tolerancja względna `DebugSettings::depth_bias`, 0.1%); slot ma rozmiar renderu, więc piksel wyjścia jest przeliczany na piksel renderu.

Kolor: cel `*Srgb` i cele zmiennoprzecinkowe dostają kolor zdekodowany do liniowego, cele `Rgba8Unorm` itp. kolor wprost, więc linia wygląda tak samo wszędzie. Mieszanie to alpha premultiplied.

## Wireframe

`WireframeFeature` (priorytet 10) rysuje siatki z `DebugDraw::wireframe`. Dwie ścieżki, ten sam bufor (trójkąty bez indeksów, każdy ze swoimi trzema wierzchołkami):

| Ścieżka | Kiedy | Wygląd |
| --- | --- | --- |
| **polygon mode line** | urządzenie ma `Features::POLYGON_MODE_LINE` (paczka prosi o nią jako opcjonalną) | linie sprzętowe 1 px, bez antialiasingu; `thickness` ignorowana |
| **barycentryczna** | brak funkcji (Web) albo `WireframeMode::Barycentric` | shader rysuje piksele blisko krawędzi z `fwidth` współrzędnych barycentrycznych, z antialiasingiem i grubością; rysuje tylko wnętrze trójkąta, więc krawędź brzegowa jest o połowę cieńsza niż wspólna |

Brak funkcji zgłasza negocjacja jako F0103 (`Fallback`), nie osobny kod. Obie ścieżki są testowane na tej samej siatce i mieszczą się w jednym pikselu od siebie. Wireframe jest ukryty za `Slot::DEPTH`, gdy ten istnieje (`DebugSettings::wireframe_depth_test`).

W RHI doszło `PolygonMode { Fill, Line }` w `PrimitiveState`; `Line` bez funkcji kończy się `RhiError::MissingFeatures` na każdym backendzie (ADR-0046).

## Podgląd slotów

`SlotPreviewFeature` (priorytet 0, przed wireframe i liniami) rysuje na `Slot::OUTPUT`: `SlotPreview::fullscreen(slot)` czyści cel, `corner(slot, Corner)` rysuje w rogu (`corner_size` = część szerokości, margines 8 px) na tym, co jest, `grid(slots)` układa wszystkie na ciemnym tle wiersz po wierszu.

| Slot | Wizualizacja |
| --- | --- |
| `depth`, `depth_pyramid` | odległość widokowa z odwróconej projekcji kamery (reverse-Z), rampa `heat` w `depth_range` (blisko = jasno), brak głębi = czerń; piramida: mip `pyramid_level` |
| `normals` | oktaedralne → wektor świata → `n·0.5+0.5` jako RGB |
| `motion` | kierunek = odcień, długość w pikselach = jasność do `motion_scale`; brak ruchu = czerń |
| `material_id` | stały kolor z hasha PCG identyfikatora |
| `roughness`, `reactive`, `ao` | skala szarości |
| `albedo` | liniowe → sRGB |
| `hdr`, `diffuse`, `specular` | `× exposure` (ustawienie albo slot `exposure` z `exposure_from_slot`), obcięte do 0…1, sRGB; NaN i Inf = magenta |
| `exposure` | skala logarytmiczna `heat`, 1.0 pośrodku |

`output` nie ma podglądu: to tam rysuje podgląd. Format źródła decyduje o jednym z trzech modułów shadera (float, głębia, uint); każdy slot czytany jest przez `textureLoad`, więc nie wymaga filtrowania.

**Kontrakt z producentami.** Podgląd niczego nie wytwarza. Slot na żądanie jest zamawiany w `prepare_view` (jak w `OnDemandConsumer` z `pathon-test`), a slot, którego nikt nie zapisał, daje szachownicę i [U0001](../diag/U0001.md) raz, dopóki nie wróci. Pozostałe sloty są czytane, gdy istnieją.

Zakresy `SlotPreview` są naprawiane i zgłaszane jako [U0003](../diag/U0003.md).

## Ustawienia (`DebugSettings`)

Czytane z `Shared` co klatkę.

| Pole | Domyślnie | Znaczenie |
| --- | --- | --- |
| `lines` | `true` | rysowanie linii (prymitywy dalej się starzeją) |
| `wireframe` | `Auto` | `Auto` = sprzętowo, gdy można; `Barycentric` wymusza fallback |
| `wireframe_depth_test` | `true` | wireframe ukryty za `Slot::DEPTH` |
| `preview` | `None` | `Option<SlotPreview>` |
| `depth_bias` | `1e-3` | względna tolerancja testu głębi linii i wireframe |

## Testy

- Jednostkowe i na `rhi-null`: `DebugDraw` (żywotność, limity, kształty), geometria, układ komórek podglądu, ustawienia, `U0001` i `U0002` w klatce bez GPU.
- GPU (`--features gpu-tests`, `tests/gpu.rs`): piksele linii względem maty (`reference::segment_coverage`: pozycja i grubość), obramowanie AABB, golden każdego slotu na znanych wzorach i siatki, wireframe `PolygonMode::Line` i fallback barycentryczny względem siebie i względem golden. Referencje: `tests/golden/debug_*.png`.

```sh
cargo test -p pathon-debug --features gpu-tests -- --test-threads=1
```
