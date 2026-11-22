# pathon-scene

Render world (M4-02, [ADR-0029](../adr/0029-render-world-i-extract.md)): to, co renderer wie o scenie — instancje, kamery i światła — w układzie `SoA`, z podwójnym buforem i zmianami przyrostowymi. Host (własny silnik albo adapter Bevy/Godot) **kopiuje** swoje dane do tego świata przez `Scene::extract`; paczki czytają niezmienną migawkę. Crate nie zna assetów, plików ani GPU: mesh i materiał to uchwyty, a ich właścicielem jest `pathon-mesh`, `pathon-material` albo host.

To zwykła paczka (`[package.metadata.pathon] kind = "pack"`): zależy tylko od `pathon-api` (reguła `pack-boundary`), więc **nie zna `pathon-mesh`** — patrz [Uchwyty zasobów](#uchwyty-zasobów).

```rust
# use pathon_api::glam::{Affine3A, Vec3};
# use pathon_api::rhi::{BufferId, Id, RawHandle};
# use pathon_scene::{CameraDesc, InstanceDesc, MaterialHandle, MeshHandle, Projection, Scene};
# fn main() -> Result<(), pathon_scene::SceneError> {
# let mesh = MeshHandle::from_id(BufferId::from_raw(RawHandle::new(0, 0)));
let mut scene = Scene::new();

// Host: raz na klatkę, na swoim wątku.
let (camera, crate_instance) = scene.extract(|writer| {
    let camera = writer.spawn_camera(&CameraDesc::look_at(
        Vec3::new(0.0, 2.0, 6.0),
        Vec3::ZERO,
        Projection::perspective(std::f32::consts::FRAC_PI_3, 0.1),
    ))?;
    let desc = InstanceDesc::new(mesh, MaterialHandle::DEFAULT, Affine3A::IDENTITY);
    Ok::<_, pathon_scene::SceneError>((camera, writer.spawn(&desc)?))
})?;

scene.extract(|writer| writer.set_transform(crate_instance, Affine3A::from_translation(Vec3::X)))?;

// Render: migawka ostatniej klatki; `Arc`, więc tanio ją przekazać dalej.
let world = scene.snapshot();
let slot = world.instances().slot_of(crate_instance).expect("live");
assert_eq!(world.instances().previous_transforms()[slot], Affine3A::IDENTITY);
assert!(world.camera(camera).is_some());
# Ok(())
# }
```

## Układ danych

- **Konwencje:** układ prawoskrętny, Y w górę, metry ([kontrakt slotów](../slots.md)); kamera i światło patrzą wzdłuż lokalnego -Z. Transformacje to `Affine3A` (obiekt → świat).
- **Instancje** (`Instances`) to `SoA`: osobny `Vec` na właściwość, indeksowany **slotem** — `transforms()`, `previous_transforms()`, `meshes()`, `materials()`, `layers()`, `flags()`, `local_bounds()`, `alive`. Konsument czyta albo wgrywa tylko kolumny, których potrzebuje. Slot martwej instancji zostaje w kolumnach; wolne sloty są ponownie używane.
- **Kamery i światła** (`Table`) są po kilka sztuk, więc trzymane po slocie jako struktury (`CameraData`, `Light`).
- **Uchwyty:** `InstanceId`, `CameraId`, `LightId` są generacyjne (`Id` z `pathon-rhi-types`: indeks + generacja). `despawn` podnosi generację slotu, więc stary uchwyt nie trafi w nowy obiekt; zapis przez taki uchwyt kończy się `N0001`.
- **Warstwy i widoczność:** `LayerMask` (maska bitowa) na instancji, kamerze i świetle; `InstanceFlags::VISIBLE` / `CASTS_SHADOWS`. `Instances::iter_visible(mask)` zwraca żywe, widoczne sloty z przecinającą się maską. Culling frustum i GPU to osobne zadania (M4-11): scena podaje `local_bounds` (`Aabb`, domyślnie `UNBOUNDED` = nigdy nie odrzucaj) i `world_bounds(slot)`.

## Uchwyty zasobów

`MeshHandle` i `MaterialHandle` to nieprzezroczyste opakowania surowego uchwytu (`RawHandle`: indeks + generacja) zasobu, którego scena nie posiada. Właściciel zasobu zachowuje własny typ i odzyskuje go bez straty:

```rust
# use pathon_api::rhi::{BufferId, Id, RawHandle};
# use pathon_scene::MeshHandle;
# let mesh_id = BufferId::from_raw(RawHandle::new(3, 1));
let handle = MeshHandle::from_id(mesh_id);    // host / adapter przy extract
let back: BufferId = handle.to_id();          // pack przy rysowaniu, tu: MeshId z pathon-mesh
assert_eq!(back, mesh_id);
```

- **Mesh:** `MeshHandle::from_id(mesh_id)` dla `pathon_mesh::MeshId`; paczka rysująca (M4-05) wraca przez `handle.to_id::<MeshId>()`. Scena nie sprawdza, czy uchwyt żyje w puli — odpowiada za to właściciel (`GeometryPool::mesh(id)` zwraca `None` dla starego).
- **Materiał:** `MaterialHandle::DEFAULT` oznacza materiał zastępczy paczki. `pathon-material` (M4-03) zdefiniuje własny `MaterialId` implementujący `Id` i mapuje tak samo.
- **Adaptery hostów** mapują własne assety (`Handle<Mesh>` Bevy, `RID` Godota) na ten sam sposób: po wgraniu zasobu do puli trzymają `host asset → MeshId`, a przy extract wołają `MeshHandle::from_id`.

## Extract i podwójny bufor

`Scene::extract(|writer| …)` pisze następny stan sceny; zwraca wartość domknięcia. `SceneWriter` widzi stan poprzedniej klatki (`writer.world()`) i go zmienia — to, czego domknięcie nie dotknie, zostaje. Metody zwracają `Result<_, SceneError>`; host sam decyduje, czy błąd pojedynczego obiektu przerywa klatkę.

- **Dwa bufory:** scena trzyma dwa `Arc<RenderWorld>`. Klatka *N* jest pisana do tylnego bufora, podczas gdy `snapshot()` wydaje przedni (*N−1*); po domknięciu bufory się zamieniają i `epoch` rośnie. Migawka jest niezmienna, więc odczyt i zapis nie mogą się wyścigać (gwarantuje to typ: zapis idzie przez `Arc::get_mut` bufora, którego nikt nie czyta).
- **Zmiany przyrostowe:** każda zmiana jest zapisana raz w tylnym buforze, a w drugim dopiero przy następnym extract (kopiowane są tylko zmienione sloty i tylko zmienione kolumny). Zapis wartości równej zapisanej jest pomijany i nie oznacza niczego (`ExtractStats::unchanged`).
- **Dirty flags i lista zmian:** `Instances::changes()` to lista slotów zmienionych przez extract, który dał tę migawkę (każdy slot raz); `Instances::dirty(slot)` mówi co (`Dirty::SPAWNED`, `DESPAWNED`, `TRANSFORM`, `MESH`, `MATERIAL`, `LAYERS`, `FLAGS`, `BOUNDS`). To lista wgrywania dla paczki: przepisz kolumny tych slotów do `GpuArray`, resztę zostaw. Konsument pamięta ostatni `epoch`; jeśli nie jest o jeden mniejszy niż bieżący (pominięte klatki), wgrywa wszystko. To samo dla `cameras().changes()` i `lights().changes()`.
- **Migawka w rękach czytelnika:** jeśli czytelnik trzyma migawkę, gdy extract chce pisać do jej bufora, scena kopiuje stan do nowego bufora (`ExtractStats::rebuilt_back_buffer`; jedna alokacja i kopia O(n)). Poprawność danych się nie zmienia. Czytelnik powinien oddać migawkę przed drugim kolejnym extract; w stanie ustalonym extract nie alokuje.
- **Panika w domknięciu:** nic nie jest publikowane, a następny extract startuje od ostatniego opublikowanego stanu.
- **Wątki:** `Scene` jest `Send`; `Snapshot` (`Arc<RenderWorld>`) jest `Send + Sync`. Scena nie tworzy wątków; host przekazuje migawkę wątkowi renderu (kanał, `Shared`) tak, jak mu wygodnie.

### Poprzednia klatka (wektory ruchu)

`previous_transforms()[slot]` to model z **poprzedniego extract** ([ADR-0027](../adr/0027-pula-geometrii-i-dane-poprzedniej-klatki.md): `ObjectTransforms::previous_model`). Nowa instancja ma `previous == model` (zero ruchu przy pojawieniu). Wiele zapisów w jednym extract zostawia poprzednią wartość sprzed tego extract. Obiekt, który w następnym extract się nie rusza, dostaje `previous = model` (zero ruchu) — to zmiana oznaczona `Dirty::TRANSFORM`, kosztuje O(ruszone), nie O(wszystkie). Kamera ma to samo: `CameraData::previous_view_projection(aspect)` (bez jittera, jak wektory ruchu w kontrakcie slotów).

## Kamery

`CameraDesc { transform, projection, layers }` (`look_at` buduje transformację). `Projection`:

| Wariant | Macierz | Głębia |
| --- | --- | --- |
| `Perspective { vertical_fov, near, far: Option }` | `far: None` = nieskończona płaszczyzna daleka | reverse-Z 0..1: near → 1, far (albo ∞) → 0 |
| `Orthographic { height, near, far }` | szerokość = `height * aspect` | reverse-Z 0..1 |

Proporcje obrazu pochodzą z widoku, który kamerę renderuje, nie z kamery: `CameraData::view_camera(&ViewCtx)` daje `pathon_api::Camera` (`world_to_view`, `projection`, `position`) z aspektem z `ViewCtx::render_size`. **Jitter** (ustawiany przez rolę `TemporalResolve`, [ADR-0022](../adr/0022-zegar-klatki-paczek.md)) to przesunięcie podpikselowe w pikselach celu renderu, y w dół; `view_camera` zamienia go na przesunięcie w clip space (`ndc = 2 * jitter / render_size`, y odwrócone), takie samo dla perspektywy i ortografii. `projection(aspect)` i `view_projection(aspect)` zwracają wersję **bez** jittera (do wektorów ruchu i reprojekcji).

## Światła

`Light` w jednostkach fizycznych ([kontrakt slotów](../slots.md)): `directional(transform, lux)`, `point(transform, lumeny)`, `spot(transform, lumeny, inner, outer)` (półkąty w radianach, `0 < inner <= outer <= π/2`). Kolor to liniowy odcień RGB (1.0 = bez zabarwienia), nie jasność. Światło świeci wzdłuż lokalnego -Z. `luminous_intensity_candela()` = lumeny / 4π (także reflektor, jak w glTF `KHR_lights_punctual`). `influence_radius()` to jawny `range` albo odległość, na której natężenie spada do `AUTO_RANGE_CUTOFF_LUX` (0,02 lx): wejście dla przypisania świateł do klastrów (M4-05).

## Diagnostyka

| Kod | Sytuacja |
| --- | --- |
| [N0001](../diag/N0001.md) | uchwyt usunięty albo nieznany |
| [N0002](../diag/N0002.md) | niepoprawna kamera (fov, near, far, wysokość) |
| [N0003](../diag/N0003.md) | niepoprawne światło (natężenie, kolor, zasięg, kąty) |
| [N0004](../diag/N0004.md) | transformacja nie jest skończona |

`SceneError::to_diagnostic()` daje `Diagnostic` z podpowiedzią.

## Budżet i testy

Budżet ([ADR-0029](../adr/0029-render-world-i-extract.md), limit w `docs/bench/budgets.toml`, [ADR-0035](../adr/0035-budzety-i-regresje.md)): extract 10 000 instancji (zarówno `spawn` wszystkich, jak i ruch wszystkich) poniżej 2 ms CPU w release. Pomiar na maszynie deweloperskiej: spawn 10 000 ≈ 1,2 ms (zawiera pierwszy wzrost kolumn), ruch 10 000 ≈ 0,19 ms, ruch 1% z 100 000 ≈ 0,04 ms, 100 000 ruchów ≈ 2,9 ms.

- `cargo test -p pathon-scene` — testy jednostkowe (uchwyty, dirty, kamery: macierze reverse-Z dla znanych punktów i zgodność z `pathon_api::Camera`, jitter, światła) oraz `tests/scene.rs`: pusta scena, migawka niezmienna podczas extract, zamiana buforów, wymuszona przebudowa, uchwyty generacyjne, brak przepisywania niezmienionych danych, `previous_model`, odzyskiwanie po panice, **losowa historia 300 extractów porównana z prostym modelem** (także: każdy zmieniony slot jest na liście zmian), skalowanie do 100 000 instancji.
- `tests/allocations.rs` — licznik alokacji: extract w stanie ustalonym (ruch 10 000, brak ruchu, `snapshot()`) = 0 alokacji.
- `cargo test -p pathon-scene --release --test budget -- --ignored --nocapture` — pomiar budżetu; `cargo bench -p pathon-scene` — `divan` dla 10 000 i 100 000.
- Przykład: `cargo run -p pathon-examples --bin scene_extract` ([przykłady](../examples.md)).
