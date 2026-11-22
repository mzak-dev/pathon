# pathon-mesh

Geometria dla każdej ścieżki rasteryzacji (M4-01, [ADR-0027](../adr/0027-pula-geometrii-i-dane-poprzedniej-klatki.md)): pula meshy w mega-buforach, layouty wierzchołków oraz dane poprzedniej klatki dla wektorów ruchu. Forward, visibility buffer i ray tracing czytają ten sam bufor: wierzchołki i indeksy są dostępne jako bufory vertex / index **i** jako storage (vertex pull), a `base_vertex` wystarcza do `draw_indexed_indirect`.

To zwykła paczka: zależy tylko od `pathon-api` (`[package.metadata.pathon] kind = "pack"`), nie wchodzi do domknięcia rdzenia i nie zna wgpu. Nie zna też sceny: instancje, kamery i materiały to `pathon-scene` (M4-02).

```rust
# use pathon_mesh::{GeometryPool, IndexData, MeshData, PoolDesc, VertexLayout};
# fn main() -> Result<(), pathon_mesh::MeshError> {
# let device = pathon_rhi::NullDevice::new();
let mut pool = GeometryPool::new(&device, "scene", VertexLayout::position_only(), PoolDesc::default());
let positions: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
let mesh = pool.upload(&device, &MeshData::new(
    "triangle",
    vec![bytemuck::cast_slice(&positions)],
    IndexData::U16(&[0, 1, 2]),
))?;
assert_eq!(pool.mesh(mesh).map(|info| info.location.base_vertex), Some(0));
# Ok(())
# }
```

## Layout wierzchołków

`VertexLayout` to opis w danych: lista **strumieni** (`VertexStream { stride, attributes }`), a w nich atrybutów (`StreamAttribute { semantic, format, offset }`). `VertexLayout::new` waliduje opis i zwraca `M0001` przy błędzie: `Position` jest obowiązkowa (`Float32x3`), `stride` i offsety są wielokrotnościami 4, atrybuty mieszczą się w `stride` i się nie nakładają, semantyka występuje raz.

- **Stałe lokacje shaderowe** zależą od semantyki, nie od układu strumieni: Position 0, Normal 1, Tangent 2, Uv0 3, Uv1 4, Color 5, Joints 6, Weights 7. Shader dla „standardowego” mesha działa więc na każdym layoucie, który ma te semantyki.
- `vertex_buffer_layouts()` daje `Vec<VertexBufferLayout>` z `pathon-rhi-types` wprost do `RenderPipelineDesc::vertex_buffers`; strumień *i* to slot wierzchołków *i*. `VertexLayout` jest `Eq + Hash`, więc wchodzi do klucza potoku ([ADR-0023](../adr/0023-permutacje-shaderow.md)).

| Layout | Strumienie |
| --- | --- |
| `position_only()` | 0: `Position` `Float32x3` (12 B) |
| `standard()` | 0: pozycje (12 B); 1: `Normal` `Float32x3` @0, `Tangent` `Float32x4` @12, `Uv0` `Float32x2` @28 (36 B) |
| `skinned()` | `standard()` + 2: `Joints` `Uint32x2` @0 (cztery `u16` po dwa w `u32`, młodszy pierwszy), `Weights` `Unorm8x4` @8 (12 B) |

Pozycje są osobno, bo prepass głębi, cienie i BLAS czytają tylko je. Formaty są nieskompresowane celowo; kompresja to inny layout, bez zmiany API.

## Pula geometrii

`GeometryPool` obsługuje jeden layout. Pamięć to **chunki**: w każdym po jednym buforze na strumień (`VERTEX | STORAGE | COPY_DST | COPY_SRC`) i bufor indeksów (`INDEX | STORAGE | COPY_DST | COPY_SRC`). Alokacja idzie w jednostkach (wierzchołki, indeksy), więc ten sam `base_vertex` adresuje każdy strumień chunka.

- **`PoolDesc`** (`#[non_exhaustive]`, `default()` + `with_*`): rozmiar chunka (`chunk_vertices`, `chunk_indices`), budżet całej puli (`max_vertices`, `max_indices`) i `frames_in_flight`. Chunk i budżet są przycinane do limitów urządzenia: `min(max_buffer_size, max_storage_buffer_binding_size)` (chunk bywa bindowany jako storage w całości) i `i32::MAX` wierzchołków.
- **`upload(device, &MeshData)`** → `MeshId` (uchwyt generacyjny). `MeshData` to jeden slice bajtów na strumień (`vertex_count * stride`) i indeksy `u16` albo `u32`. Indeksy są w puli **zawsze `u32` i względne** wobec mesha: shader dodaje `base_vertex`. Pula liczy granice (`MeshInfo::bounds`) z pozycji. Nieudany upload nie zostawia zajętych zakresów ani chunków.
- **`MeshLocation`** (z `mesh(id)`): `vertex_chunk`, `base_vertex`, `vertex_count`, `index_chunk`, `first_index`, `index_count`. Offsety nie są stałe: nic poza pulą ich nie trzyma, więc pula może je w przyszłości przesuwać (defragmentacja) bez zmian API.
- **Rysowanie:** `draw(pass, mesh, instances)` wiąże bufory chunka i woła `draw_indexed`; `bind(pass, mesh)` tylko wiąże i zwraca lokalizację (sortuj rysowania po chunku i wiąż raz). Dla vertex pull: `vertex_buffers(chunk)` i `index_buffer(chunk)` dają bufory do bind group (storage: `array<f32>` / `array<u32>` o indeksie `(base_vertex + index) * stride / 4`).
- **Zwalnianie:** `remove(mesh, frame)` unieważnia uchwyt od razu, ale zakres wraca do alokatora dopiero w `begin_frame(device, frame)` po `frames_in_flight` klatkach ([ADR-0022](../adr/0022-zegar-klatki-paczek.md)); lista nagrana, a niezgłoszona (tryb A), może jeszcze czytać zakres. Pusty chunk (poza ostatnim standardowym) jest niszczony, a chunk dedykowany dla dużego mesha — zawsze. Mesh większy niż `chunk_vertices` dostaje własny chunk do limitu.
- **Fragmentacja:** alokacja first-fit ze scalaniem w obrębie chunka (`RangeAllocator`), chunki po kolei. `stats()` podaje per strumień `capacity`, `used`, `pending`, `free`, `largest_free` i `fragmentation()` (0 = jedno wolne miejsce, w stronę 1 = dziury), żeby paczka ostrzegała przed błędem.
- **Utrata urządzenia:** `forget_gpu()` unieważnia wszystkie uchwyty (pula nie trzyma kopii CPU), a aplikacja wgrywa meshe od nowa; `release_all(device)` niszczy bufory.

### Diagnostyka

| Kod | Sytuacja |
| --- | --- |
| [M0001](../diag/M0001.md) | niepoprawny layout wierzchołków |
| [M0002](../diag/M0002.md) | dane mesha nie pasują do layoutu (długość strumienia, liczba wierzchołków, pusty mesh, indeks poza zakresem) |
| [M0003](../diag/M0003.md) | budżet puli wyczerpany |
| [M0004](../diag/M0004.md) | łącznie jest miejsce, ale brak ciągłego zakresu (fragmentacja albo zwolnienia czekają na klatki w locie) |
| [M0005](../diag/M0005.md) | mesh większy niż jeden bufor puli |
| [M0006](../diag/M0006.md) | nieznany albo usunięty mesh |

Błędy urządzenia przechodzą jako `MeshError::Rhi` z kodem `R…`. `MeshError::to_diagnostic()` daje `Diagnostic` z podpowiedzią.

## Dane poprzedniej klatki

Wektory ruchu (kontrakt slotów) potrzebują pozycji sprzed jednej klatki. Układ jest ustalony tu, żeby `pathon-scene` (M4-02), prepass (M4-05) i skinning (M4-12) go nie zgadywały.

- **Transformacje obiektów:** `TransformHistory` trzyma jeden storage buffer par `ObjectTransforms { model, previous_model }` (96 B, `AffineRows` = trzy wiersze `vec4`). WGSL: `struct ObjectTransforms { model: mat3x4<f32>, previous_model: mat3x4<f32> }`; punkt `p` (w = 1) to `vec3(dot(r0, p), dot(r1, p), dot(r2, p))` albo `p * m`. `begin_frame(frame)` (raz na klatkę, idempotentne) przepisuje `model` do `previous_model` tylko dla obiektów ruszonych w poprzedniej klatce; nowy obiekt nie ma ruchu. `sync` wysyła tylko zmieniony zakres (`GpuArray`).
- **Pozycje po skinningu (layout dla M4-12):** `DeformedPositions` to jeden storage buffer (`STORAGE | VERTEX`) z **dwoma bankami** po `bank_capacity` wierzchołków `vec4<f32>` (xyz w przestrzeni obiektu, w = 1; 16 B). Instancja skinowana dostaje raz `DeformedRange { first_vertex, vertex_count }`, ważny w obu bankach. W klatce `f` skinning pisze bank `f & 1` (`current_offset`), a wektory ruchu czytają bank `(f & 1) ^ 1` (`previous_offset`); adres w shaderze to `bank * bank_capacity + first_vertex + vertex_id`. Zakresy zwalnia się przez `free(range, frame)` + `begin_frame(frame)`, tak jak meshe.
- **Strumień skinningu** to strumień 2 layoutu `skinned()` (tabela wyżej). Normalne i tangenty po skinningu M4-12 pisze do osobnego bufora jednobankowego o układzie strumienia 1.

Klatki podaje wołający (`ViewCtx::frame`); crate nie czyta zegara i nie tworzy wątków.

## Testy i przykład

- `crates/pathon-mesh/tests/pool.rs` (`rhi-null`): alokacje bez nakładania (także pod losowym churnem), odczyt danych z buforów, reuse po `frames_in_flight`, stary uchwyt nie trafia w nowy slot, granice (mesh równy chunkowi, o jeden większy, większy niż budżet, pusty), `M0003`…`M0006`, rollback nieudanego uploadu, nagrane komendy `draw`, `DeformedPositions`, `TransformHistory`.
- `crates/pathon-mesh/tests/draw_from_one_buffer.rs`: trzy meshe z jednego chunka rysowane offscreen, odczyt pikseli, potem usunięcie jednego i reuse jego zakresu. Na GPU z `--features gpu-tests` (`cargo test -p pathon-mesh --features gpu-tests -- --test-threads=1`), na `rhi-null` zawsze (walidacja komend).
- Przykład: `cargo run -p pathon-examples --bin mesh_pool` ([przykłady](../examples.md)).
