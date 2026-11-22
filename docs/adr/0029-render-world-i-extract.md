# ADR-0029: Render world, extract i podwójny bufor

**Status:** Przyjęta
**Data:** 2026-11-22
**Dotyczy:** [ADR-0004](0004-model-watkow.md) (wątki), [ADR-0005](0005-hosty-i-tryby-integracji.md) (hosty), [ADR-0022](0022-zegar-klatki-paczek.md) (zegar klatki, jitter), [ADR-0027](0027-pula-geometrii-i-dane-poprzedniej-klatki.md) (`previous_model`)

## Kontekst

Wszystkie paczki M4–M5 (prepass, forward+, cienie, culling GPU, skinning) czytają to samo: instancje z transformacją i poprzednią transformacją, kamery, światła. Kształt tych danych i sposób, w jaki host je dostarcza, jest drogi do zmiany po tym, jak powstanie na nim kilka paczek i adaptery Bevy i Godota (M7). Host ma własną scenę (ECS Bevy, drzewo węzłów Godota, własny silnik), więc renderer nie może jej współdzielić: musi mieć własną, kopiowaną z hosta kopię — render world — i tanio ją aktualizować, także gdy host i render pracują równolegle (tryb C, ADR-0004).

Reguła `pack-boundary` ([deps-check](../deps-check.md)) pozwala paczce zależeć z crate'ów pathon tylko od `pathon-api` i `pathon-shader`. `pathon-mesh` jest paczką, więc `pathon-scene` nie może go używać; `MeshId` jest w `pathon-mesh`, a `pathon-resources` nie ma typu materiału.

## Decyzja

### Granica crate'u i uchwyty zasobów

- `pathon-scene` jest zwykłą paczką (`kind = "pack"`): zależy tylko od `pathon-api` (plus `bitflags`, `thiserror`). **Reguły `deps-check` nie zmieniamy.** Nie ma w niej krawędzi `pathon-scene → pathon-mesh`.
- Mesh i materiał to `MeshHandle` i `MaterialHandle`: opakowania `RawHandle` (indeks + generacja z `pathon-rhi-types`, dostępne przez `pathon_api::rhi`). Mapowanie nie wymaga zależności, bo oba końce znają trait `Id`: `MeshHandle::from_id(mesh_id)` zapisuje `mesh_id.raw()`, a `handle.to_id::<MeshId>()` odtwarza `MeshId` przez `Id::from_raw`. Adapter hosta albo paczka rysująca, które znają `pathon-mesh`, robią to bez pośrednika; scena zachowuje tylko opaque uchwyt.
- `MaterialHandle::DEFAULT` (indeks `u32::MAX`) to materiał zastępczy paczki. `pathon-material` (M4-03) definiuje własny `MaterialId: Id` i mapuje tak samo; osobnego typu `TextureHandle` scena nie wprowadza, dopóki coś w niej nie czyta tekstur (światła z cookie, środowisko: M4-06, M4-13).
- Scena nie weryfikuje, czy uchwyt mesha żyje w puli: to odpowiedzialność właściciela (`GeometryPool::mesh(id)` zwraca `None` dla starego). Uchwyty `InstanceId`, `CameraId`, `LightId` są generacyjne i scena odrzuca stare (`N0001`).

### Układ: SoA dla instancji, tablice po slocie dla reszty

- Instancje to **structure of arrays** (kolumny: `generation`, `alive`, `transform`, `previous_transform`, `mesh`, `material`, `layers`, `flags`, `local_bounds`, `dirty`), indeksowane slotem; wolne sloty wracają na stos i są używane ponownie, `despawn` podnosi generację. Kamery i światła (rzędu jednostek) to `Table<I, T>` — wektor `Option<T>` po slocie z tą samą generacją i listą zmian.
- Transformacja to `glam::Affine3A` (obiekt → świat; 48 B). Układ GPU (`ObjectTransforms`, wiersze `mat3x4`) należy do `pathon-mesh` (ADR-0027); paczka wgrywająca konwertuje kolumny sceny. Scena sama nic nie wgrywa na GPU.
- `LayerMask` (`u32`) i `InstanceFlags` (`VISIBLE`, `CASTS_SHADOWS`) dają widoczność i warstwy; `Aabb` w przestrzeni mesha (`UNBOUNDED` = nieznane, nigdy nie odrzucaj) — wejście dla cullingu (M4-11). Frustum culling nie należy do tego zadania.

### Podwójny bufor i extract

- `Scene` trzyma dwa `Arc<RenderWorld>`: przedni (ostatni opublikowany) i tylny. `Scene::extract(|writer| …)` pisze do tylnego przez `Arc::get_mut`, po czym zamienia bufory i podnosi `epoch`. `Scene::snapshot()` zwraca `Arc` przedniego; migawka jest niezmienna. Wyścig odczytu z zapisem jest więc **wykluczony typami**, bez blokad: zapis nigdy nie dotyka bufora, który ktoś trzyma.
- Zapis jest przyrostowy. Writer zapisuje wartość tylko gdy różni się od zapisanej (porównanie `==`); każdy zapis oznacza slot w `dirty` (flagi `SPAWNED`, `DESPAWNED`, `TRANSFORM`, `MESH`, `MATERIAL`, `LAYERS`, `FLAGS`, `BOUNDS`) i dopisuje go raz do listy `changes`. Drugi bufor nadrabia przy następnym extract (**catch-up**): kopiuje z przedniego tylko sloty z listy i tylko oznaczone kolumny (spawn i despawn: wszystkie). Każda zmiana jest więc wykonana dwa razy (raz na bufor), nigdy nie przepisujemy tego, co się nie zmieniło.
- `changes` + `dirty` + `epoch` to kontrakt dla konsumenta GPU: wgrywa tylko zmienione sloty; gdy `epoch` nie jest o jeden większy od ostatnio widzianego (pominięte klatki), wgrywa wszystko. Lista zmian dotyczy jednego extract; scena nie kumuluje ich na życzenie spóźnionego czytelnika.
- Czytelnik, który trzyma migawkę, gdy extract chce pisać do jej bufora, nie blokuje i nie psuje danych: scena buduje nowy tylny bufor pełną kopią przedniego (`ExtractStats::rebuilt_back_buffer`). W stanie ustalonym (czytelnik oddaje migawkę przed drugim kolejnym extract) extract **nie alokuje** (test z licznikiem alokacji).
- Panika w domknięciu `extract` nic nie publikuje; następny extract odbudowuje tylny bufor i listy wolnych slotów z ostatniego opublikowanego stanu.
- Scena nie tworzy wątków i jest `Send`; host przekazuje migawkę wątkowi renderu (kanał, `Shared`) po swojej stronie. Dlatego nie ma tu blokad ani kanałów, a wyścigi wykluczają typy.

### Poprzednia klatka

- `previous_transform` to model z **poprzedniego extract**. Invariant: na początku extract każda żywa instancja ma `previous == model`. Zapisy w trakcie extract zmieniają tylko `model`, więc poprzednią wartością jest stan sprzed extract (także przy wielu zapisach). Nowa instancja: `previous == model`.
- Żeby invariant zachodził bez przeglądania wszystkich instancji, na początku extract (**settle**) sloty z `Dirty::TRANSFORM` w poprzednim extract dostają `previous = model` (O(ruszone)); ten zapis jest zmianą (`TRANSFORM`) i trafia na listę wgrywania. Obiekt, który przestał się ruszać, kończy więc z zerowym ruchem, tak samo jak w `TransformHistory` (ADR-0027). Kamery: ten sam mechanizm dla `previous_transform` i `previous_projection`.

### Kamery

- `Projection::Perspective { vertical_fov, near, far: Option }` (`None` = nieskończona płaszczyzna daleka) i `Orthographic { height, near, far }`, obie **reverse-Z 0..1** (near → 1): perspektywa daje `clip.z = a * z + b`, `a = near / (far - near)`, `b = near * far / (far - near)` (`a = 0`, `b = near` dla nieskończonej), `clip.w = -z`. Test porównuje nieskończoną wersję z `pathon_api::Camera::look_at`.
- Aspekt nie należy do kamery, tylko do widoku: `CameraData::view_camera(&ViewCtx)` bierze go z `render_size`. **Jitter** z `ViewCtx::jitter` to przesunięcie podpikselowe w pikselach celu renderu (y w dół, jak współrzędne obrazu), stosowane jako mnożenie przez translację w clip space (`ndc = 2 * jitter / render_size`, y odwrócone), więc tak samo dla perspektywy i ortografii. Wektory ruchu i reprojekcja używają wersji bez jittera (kontrakt slotów).
- Transformacja kamery to kamera → świat; widok to jej odwrotność (kamera i światło patrzą wzdłuż lokalnego -Z).

### Światła

- `Light { kind, transform, color, intensity, range, casts_shadows, layers }` w jednostkach fizycznych: kierunkowe w **lux** (illuminance), punktowe i reflektor w **lumenach** (moc świetlna); kolor to liniowy odcień RGB, nie jasność. Natężenie w kandelach to lumeny / 4π także dla reflektora (jak glTF `KHR_lights_punctual`: stożek kształtuje światło, nie koncentruje mocy).
- Brak `range` wylicza `influence_radius()` z progu `AUTO_RANGE_CUTOFF_LUX = 0.02` lx (prawo odwrotnych kwadratów). Próg jest strojony razem z przypisaniem do klastrów (M4-05); zmiana wartości nie zmienia API.

### Diagnostyka

Prefiks **N** (scene): `N0001` nieaktualny uchwyt, `N0002` niepoprawna kamera, `N0003` niepoprawne światło, `N0004` wartość nieskończona lub `NaN`. Strony w `docs/diag/`. Nowe publiczne struktury z opisami (`InstanceDesc`, `CameraDesc`, `CameraData`, `Light`, `ExtractStats`) mają `#[non_exhaustive]` i konstruktory (ADR-0020).

### Tymczasowy budżet extract (do przeniesienia w M4-17)

Budżet z M4-17 jeszcze nie istnieje, więc zadanie ustala wartość tymczasową: **extract 10 000 instancji < 2 ms CPU w release** (jeden wątek, mediana 41 przebiegów), osobno dla `spawn` wszystkich i dla ruchu wszystkich. Pilnuje go test `crates/pathon-scene/tests/budget.rs` (`#[ignore]`, uruchamiany ręcznie w release) oraz benchmark `divan`. Deterministycznie pilnują kosztu: test zero alokacji w stanie ustalonym i test, że extract bez zmian nie zapisuje ani nie oznacza niczego. Pomiar na maszynie dewelopera (22.11.2026): spawn 10 000 ≈ 1,2 ms, ruch 10 000 ≈ 0,19 ms, nic się nie zmieniło ≈ 0,08 ms; przy 100 000: spawn ≈ 9,3 ms, ruch ≈ 2,9 ms, ruch 1% ≈ 0,04 ms, bez zmian ≈ 0,8 ms. M4-17 zastępuje stałą 2 ms wartością z tabeli budżetów.

## Rozważane warianty

- **Zależność `pathon-scene → pathon-mesh` (użycie `MeshId` wprost) z poluzowaniem `pack-boundary`:** rozmywa regułę, która ma trzymać paczki niezależne; mapowanie przez `Id::raw` kosztuje jedną konwersję i nic nie tracimy.
- **Jedna kopia sceny + kopiowanie całości do migawki co klatkę:** O(n) niezależnie od liczby zmian; przy 100 000 instancji to megabajty kopiowane co klatkę.
- **`Mutex` / `RwLock` na świecie:** renderer czekałby na extract albo odwrotnie; migawki `Arc` dają to samo bez blokad i z gwarancją niezmienności.
- **Log operacji zamiast kopiowania slotów przy catch-up:** wymaga przechowywania wartości każdej operacji i odtwarzania logiki (settle, generacje); kopiowanie kolumn z przedniego bufora jest prostsze i nie może się rozjechać.
- **Pełny ECS (`bevy_ecs`, `hecs`) jako render world:** zależność, której rdzeń nie ma (ADR-0015), a wzorzec dostępu (kolumny po slocie, lista zmian) jest tu prostszy niż archetypy.
- **Aspekt w kamerze:** nieaktualny po zmianie rozmiaru celu i błędny przy wielu widokach jednej kamery.

## Konsekwencje

- Hosty wołają `extract` raz na klatkę i mapują własne encje na `InstanceId`/`CameraId`/`LightId` (adapter trzyma tę mapę); `N0001` oznacza błąd adaptera.
- Paczka wgrywająca instancje (M4-05) czyta `changes`/`dirty`/`epoch` i konwertuje kolumny na `ObjectTransforms`; nie przepisuje całości.
- Zmiana układu kolumn instancji, semantyki `previous_transform` albo konwencji jittera po powstaniu paczek konsumentów wymaga nowego ADR.
- Czytelnik, który zatrzymuje migawkę dłużej niż jeden extract, płaci jedną kopią O(n) na extract (nie błędem); widoczne w `ExtractStats::rebuilt_back_buffer`.
- Pamięć: dwa komplety kolumn (ok. 150 B na instancję na bufor).
- Stała 2 ms jest wartością tymczasową; M4-17 ją zastępuje.
