# pathon-skin

Skinning i morph targets w compute (M4-12, [ADR-0043](../adr/0043-skinning-i-morph-targets-w-compute.md)): `SkinPack` deformuje instancje meshy w jednym passie na początku klatki i zapisuje pozycje, normalne i tangenty do buforów, które czytają prepass, cienie i forward. Pozycje trafiają do `DeformedPositions` z [pathon-mesh](pathon-mesh.md) w banku bieżącej klatki, a bank drugi zostaje z pozą poprzedniej klatki, z której TAA liczy wektory ruchu (kontrakt ADR-0027).

To zwykła paczka (`[package.metadata.pathon] kind = "pack"`): zależy od `pathon-api` i `pathon-mesh` (`pack-lib`, ADR-0041), jeden shader WGSL na każdym profilu, także w przeglądarce. Nie liczy animacji (klipy, hierarchia, odwrotne macierze bindu to sprawa aplikacji albo hosta) i nie rysuje.

```rust,ignore
let mut renderer = Renderer::builder().with_pack(SkinPack::new()).build()?;
let device = renderer.device().clone();

// Raz: mesh o layoucie skinned() w puli i miejsce na wynik.
let mut pool = GeometryPool::new(&*device, "postacie", VertexLayout::skinned(), PoolDesc::default());
let mesh = pool.upload(&*device, &mesh_data)?;
let source = SkinSource::from_pool(&pool, mesh)?;
let mut positions = DeformedPositions::new(&*device, "pozycje", 65_536, 2)?;
let surface = DeformedSurface::new(&*device, "powierzchnia", 65_536)?;
let range = positions.allocate(source.vertex_count)?;
let targets = SkinTargets::new(&positions, &surface)?;

// Co klatkę: macierze skinningu i instancje.
let mut frame = SkinFrame::new(targets);
frame.bone_matrices.extend(skeleton.skinning_matrices());
frame.instances.push(SkinInstance::new(source, range).with_bones(0, bone_count).continuing());
renderer.shared_mut().insert(frame);
renderer.render_frame(&input)?;
```

## Wejście: `SkinFrame`

Aplikacja wkłada `SkinFrame` do `Shared` przed `render_frame` i podmienia go w każdej klatce, w której instancje mają być deformowane (`Shared::get_mut` pozwala odzyskać wektory bez alokacji).

| Pole | Znaczenie |
| --- | --- |
| `targets` | `SkinTargets { positions, surface, bank_capacity }` z `SkinTargets::new(&DeformedPositions, &DeformedSurface)`; pojemności muszą się zgadzać (`W0003`) |
| `bone_matrices` | macierze skinningu w przestrzeni obiektu (poza stawu pomnożona przez odwrotną macierz bindu), `Mat4` kolumnami; instancje wskazują plasterki, więc wspólny szkielet albo paleta na instancję |
| `morph_weights` | wagi wszystkich celów morphów wszystkich instancji w jednym wektorze |
| `instances` | `SkinInstance` |

`SkinInstance::new(source, range)` + `with_bones(first, count)` + `with_morph(targets, first_weight)` + `continuing()`:

- **`SkinSource`** (`from_pool(&pool, mesh)`): bufory chunka mesha i `base_vertex`; pula ma layout `skinned()` (kości + opcjonalnie morphy) albo `standard()` (tylko morphy, bez kości). Instancja ze strumieniem skinningu musi mieć kości i odwrotnie (`W0001`).
- **`DeformedRange`** z `DeformedPositions::allocate(mesh.vertex_count)`; zakres obowiązuje w obu bankach i w `DeformedSurface`. Zwalnia się go jak mesh: `free(range, frame)` + `begin_frame(frame)`.
- **`MotionHistory`**: `Fresh` (domyślnie) w pierwszej klatce instancji, po pominiętej klatce i po cięciu kamery: poza jest pisana do obu banków, ruch wynosi zero. `Continuing` od drugiej klatki, gdy instancja była deformowana w klatce poprzedniej. Błąd w tym miejscu daje smużenie, nie błąd walidacji.

## Co robi shader

Jeden dispatch na instancję, wątek na wierzchołek, kolejność jak w `reference::deform_vertex`:

1. pozycja, normalna i tangent z puli (vertex pull, bez kopii);
2. **morphy**: `p += Σ wₖ · Δpₖ`, tak samo normalna i tangent (delty z wagą 0 są pomijane; wagi mogą być ujemne i większe od 1, to wołający odpowiada za zakres);
3. **kości**: `M = Σ wᵢ Mᵢ / Σ wᵢ` po wpływach z wagą > 0 i indeksem poniżej liczby kości instancji. Suma Unorm8 różna od 1 jest renormalizowana, indeks spoza palety pomijany, a wierzchołek bez wpływu zostaje w pozie po morphach;
4. `p' = M · p`, normalna i tangent przez górną macierz 3×3 `M` (jak glTF; nierównomierna skala w kościach nie jest korygowana) i normalizacja (zerowy wektor zostaje zerowy: bez NaN);
5. zapis `vec4(p', 1)` do banku `frame & 1` (przy `Fresh` także do drugiego) i `Normal`, `Tangent` (w = parzystość z mesha), `Uv0` do `DeformedSurface`.

Morphy są **gęste**: `MorphTargets::upload(device, label, vertex_count, &[MorphTarget])` układa `[cel][wierzchołek][Δpozycja, Δnormalna, Δtangent]` w jednym buforze storage; pusty strumień delt to zera. Wersja sparse (lokalne kształty) zmieni tylko ten bufor; nie jest zrobiona, bo wymaga wyszukiwania po wierzchołku. Pamięć: 48 B na parę (cel, wierzchołek).

## Wynik: `SkinOutput` i bufory do rysowania

Po klatce, w której coś deformowano, `Shared` ma `SkinOutput { positions, surface, bank_capacity, frame }` (uchwyty grafu po passie compute). Pass, który czyta wynik, robi `read_buffer(output.positions, Access::VERTEX)`; graf ułoży go po passie `skin` i wpisze barierę. `SkinOutput` jest ustawiany w każdym widoku klatki, a pass `skin` jest w grafie raz (w pierwszym widoku próby nagrania); bez instancji `SkinOutput` znika z `Shared`.

Do rysowania: bank bieżący `DeformedPositions` (offset `current_offset(range, frame)`) jako strumień 0 (`Float32x4`, stride 16, lokacja 0) i `DeformedSurface` (offset `DeformedSurface::offset(range.first_vertex)`) jako strumień 1 (normalna 1, tangent 2, uv 3, stride 36); `deformed_vertex_buffers()` daje oba `VertexBufferLayout`. Indeksy zostają w puli (względne, `base_vertex` 0).

**Wektory ruchu:** pozycja poprzedniej klatki to ten sam wierzchołek w banku `previous_offset(range, frame)`; różnica `current − previous` (`reference::motion`) to ruch w przestrzeni obiektu, a ruch całego obiektu dokłada `previous_model` z `TransformHistory`.

## Kolejność w grafie i wiele widoków

Pass `skin` leży w `Stage::Setup` z priorytetem −10, więc poprzedza cienie, prepass i „clear output”. Dwa widoki jednej klatki nie deformują dwa razy: pass jest w grafie pierwszego widoku próby nagrania (`ViewCtx::record_epoch`), a dalsze widoki importują bufory w stanie spoczynku (`VERTEX | STORAGE_READ`) i publikują te same bufory w `SkinOutput`.

## Referencja CPU i dane testowe

`pathon_skin::reference` powtarza obliczenia shadera (`deform_vertex`, `blended_matrix`, `Influences` z pakowaniem `Joints` / `Weights`, `motion`); testy porównują z nim GPU, a aplikacja może sprawdzić nim własne dane. `SampleArm` to ramię z dwoma kośćmi (walec wzdłuż +X, łokieć w `x = 1`, wagi mieszane wokół łokcia, `SampleArm::pose(kąt)` daje macierze skinningu): jedyna geometria, której używają testy i przykład (jak `SyntheticSky` w `pathon-ibl`).

## Diagnostyki

| Kod | Sytuacja |
| --- | --- |
| [W0001](../diag/W0001.md) | instancja nie pasuje do buforów (zakres, kości, wagi, layout puli); jest pomijana |
| [W0002](../diag/W0002.md) | niepoprawne dane morph targets |
| [W0003](../diag/W0003.md) | bufory wynikowe nie pasują do urządzenia albo do siebie |

`SkinError::to_diagnostic()` daje `Diagnostic` z podpowiedzią. `negotiate` wyłącza paczkę (`F0101`) bez grup compute 64 wątków; po utracie urządzenia `setup` tworzy potoki od nowa, a aplikacja alokuje bufory wynikowe i morphy ponownie.

## Granice

- Nie liczy animacji, IK, blend shape'ów z korektami ani deformacji wektorów prędkości w przestrzeni świata.
- Prepass i cienie `pathon-pbr` / `pathon-shadow` jeszcze nie czytają `SkinOutput` (podpięcie to osobna zmiana, ADR-0043); do tego czasu deformowane meshe rysuje własny pass aplikacji.
- Test z TAA (animowany model bez smużenia) czeka na M5-01; tu jest dowód kontraktu.

## Testy i przykład

- Testy jednostkowe: referencja CPU (obrót, renormalizacja wag, morphy, pakowanie), układ bindingów i struktury parametrów względem refleksji naga, dane morphów, bufory wynikowe, ramię.
- `tests/null_pack.rs` (`rhi-null`, zawsze): renderer z paczką waliduje bariery, flagi użycia i grupy wiązań; dwa widoki dają jeden pass; pass czytający wynik jest po `skin` z barierą; zła instancja jest pomijana i zgłaszana raz.
- `tests/gpu_contract.rs` (`--features gpu-tests`, `PATHON_TEST_BACKEND=vulkan`): kość obraca się między klatkami: pozycje `current` i `previous` zgadzają się z CPU (±1e-4), różnica z wektorem analitycznym; `Fresh` daje zerowy ruch; wagi morphów (także ujemne i > 1) i liniowość; mesh bez strumienia skinningu; normalne i tangenty po deformacji; wagi nieznormalizowane, zerowe i indeks spoza palety bez NaN; wynik identyczny co do bitu przy zmianie miejsca i długości palety.
- `tests/golden_arm.rs`: ramię w trzech pozach narysowane z buforów wynikowych (normalne jako kolor), golden `skin_arm` (job `golden` w CI).
- Przykład: `cargo run -p pathon-examples --bin skin_arm -- wynik.png` ([przykłady](../examples.md)).
