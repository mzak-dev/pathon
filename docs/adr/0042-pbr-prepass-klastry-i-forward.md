# ADR-0042: PBR — prepass, klastry świateł w compute i forward

**Status:** Przyjęta
**Data:** 2026-11-25
**Dotyczy:** M4-05 (`pathon-pbr`), [ADR-0041](0041-pack-lib.md), [ADR-0029](0029-render-world-i-extract.md), [ADR-0030](0030-material-standardowy-i-hooki-wgsl.md), [ADR-0036](0036-bindless-i-bind-group-per-material.md), [ADR-0037](0037-ibl-srodowisko-prefiltr-i-lut.md), [ADR-0047](0047-uzywanie-kodu-filamenta.md)

## Kontekst

M4-05 składa z gotowych elementów (scena, pula meshy, materiał, bindless, IBL) pierwszy obraz sceny: sloty prepassu, przypisanie świateł do klastrów i pass koloru. Do rozstrzygnięcia: skąd paczka bierze dane (reguła `pack-boundary` zabrania zależności od innych paczek, ADR-0041 dopuszcza `pack-lib`), jak przypisywać światła bez atomików 64-bit, kto zajmuje się jitterem i poprzednią macierzą kamery, jak dzielić `diffuse` i `specular`, jak zachować się bez środowiska i bez bindless.

Wzorem wyglądu jest Filament, ale **bez jego kodu** (ADR-0047): wszystkie wzory pochodzą z publikacji (Karis 2013, Heitz 2014, Burley 2012, Kulla i Conty 2017, Lagarde i de Rousiers 2014, Jimenez i in. 2016, Olsson i in. 2012, specyfikacja glTF `KHR_lights_punctual`), a jakość mierzymy porównaniem obrazów.

## Decyzja

### Trzy klasy, jeden stan

`PbrPack` instaluje `PrepassFeature` (etap `Prepass`), `DepthPyramidFeature` (`Prepass`, po prepassie) i `LightingFeature` (`Opaque` i `Transparent`, po prepassie). Dzielą stan w `Shared` (`PbrContext`: shadery i potoki, bufor roboczy, potok klastrów, przekazanie danych widoku z prepassu do koloru, historia kamery). Prepass jako pierwszy czyta scenę i pisze dane klatki; kolor tylko je zużywa. Klasy da się więc wyłączyć osobno (pyramid jest na żądanie), a `LightingFeature` bez prepassu nic nie rysuje.

### Wejścia przez `Shared`, bez zależności od właściciela

Paczka czyta `pathon_scene::Snapshot` (co klatkę), `GeometryPool`, `MaterialTable` (wymagane, brak to `L0004`), `TextureBinder`, `MaterialHooks` i `IblEnvironment` (opcjonalne). Aplikacja wstawia je przez `Renderer::shared_mut()` po zbudowaniu renderera, bo wymagają urządzenia; dlatego shadery i potoki powstają **leniwie** przy pierwszym rysowaniu (hook `texture_source` zależy od binderu), a `setup` niczego nie kompiluje. Paczka sama woła `MaterialTable::sync` i `TextureBinder::prepare` (z własną kolejką `DeferredRelease`), więc aplikacja nie musi ich pamiętać. `MaterialId` implementuje teraz `Id` (indeks bez generacji), więc `MaterialHandle::from_id` i `to_id` działają; `MaterialHandle::DEFAULT` oznacza materiał o indeksie 0. Doszły `TextureTable::handle_for_value` i `TextureBinder::bind_group_for_values`: paczka ma tylko `MaterialData`, a ścieżka per-materiał potrzebuje uchwytów.

### Sloty prepassu

| Slot | Zapis |
| --- | --- |
| `depth` | `Depth32Float`, reverse-Z, czyszczone do 0; pozycja `@invariant`, żeby kolor mógł testować `GreaterEqual` bez zapisu |
| `normals` | normalna **shadingu** (z mapą normalnych, po odwróceniu dla tylnej ściany) w świecie, kodowanie oktaedralne w `Rg16Float` |
| `motion` | piksele, `poprzedni − bieżący` (bieżący piksel + ruch = gdzie punkt był), y w dół, **bez jittera**; z interpolowanych pozycji clip: bieżącej z `unjittered_view_projection` i poprzedniej z `previous_model` i poprzedniej macierzy kamery |
| `material_id` | indeks w `MaterialTable`; tło ma `u32::MAX` |
| `roughness`, `albedo` | na żądanie (`PrepareCtx::request`), razem; albedo to kolor bazowy w RGB i `metallic` w alfa |
| `depth_pyramid` | na żądanie, `R32Float` z mipami, minimum (czyli najdalsza głębia) bloku; mip o nieparzystym rozmiarze źródła bierze dodatkowy wiersz i kolumnę, żeby nic nie wypadło; jeden pass compute na mip |

Prepass liczy `load_surface` dla każdego materiału (potrzebuje normalnej, chropowatości i albedo), więc maskowane materiały odrzucają w nim fragmenty, a kolor widzi tę samą głębię. Pipeline'y prepassu to dwa warianty (opaque, masked), kolor ma wariant na każdą permutację materiału.

### Jitter i poprzednia kamera należą do paczki geometrii

`ViewInput::camera` jest **bez jittera**; paczka nakłada `ViewCtx::jitter` (przesunięcie w clip space, jak w `CameraData::view_camera`) na projekcję do rasteryzacji i klastrów, a wektory ruchu liczy z projekcji bez jittera. Poprzednią macierz kamery trzyma per `ViewId`, przesuwaną tylko gdy zmieni się `ViewCtx::frame` (klatka nagrana ponownie nie nadpisuje historii). Host, który sam nakłada jitter (np. przez `view_camera`), nie może go nakładać drugi raz.

### Klastry w compute, bez atomików 64-bit

Siatka: kafle `tile_size` px (domyślnie 16) razy `slices` (24) warstw głębi, wykładniczych od `near` (0,1 m) do `far` (100 m); warstwa 0 bierze też wszystko bliżej, ostatnia wszystko dalej (granice AABB: 0 i `LAST_SLICE_END`). Jeden wątek na klaster liczy AABB w przestrzeni widoku (promień piksela przecięty z płaszczyzną głębi, więc działa dla perspektywy i ortografii), zlicza światła, których sfera zasięgu dotyka AABB, rezerwuje fragment wspólnej listy **jednym `atomicAdd` na `u32`** i zapisuje indeksy w kolejności świateł. Przepełnienie listy skraca listy późniejszych klastrów i jest liczone (licznik 1); lista, nie atomik, jest więc jedynym limitem (`max_light_indices`, `L0005`). Światła kierunkowe są poza klastrami i stoją na początku tablicy. Zasięg to `Light::influence_radius`, a okno tłumienia (Karis 2013: `saturate(1 − (d/r)⁴)²`) zeruje światło dokładnie na zasięgu, więc odrzucenie przez klaster niczego nie zmienia w obrazie: test sprawdza równość z trybem `BruteForce` (każde światło) na tysiącu świateł.

Koszt `O(klastry × światła)` jest znany: hierarchiczne przycinanie (kafle, potem warstwy) jest otwarte, do zmierzenia w M4-17.

### Kolor, jednostki, `diffuse` i `specular`

Światła w jednostkach fizycznych (lux dla kierunkowych, kandela dla punktowych i reflektorów = lumeny / 4π jak w glTF); `hdr` jest liniowe i bez ekspozycji (kontrakt `post`). Pętla świateł woła `material::standard::evaluate`, środowisko `ibl::shading::ibl_evaluate`; okluzja materiału zaciemnia dyfuzję IBL, a specular dostaje okluzję spekularną Lagarde'a (2014); oba mnożone korektą wielokrotnego odbicia Jimenez i in. (2016). Cienie to hak WESL `pbr::hooks::shadow_factor(light_index, position)` zwracający 1 (M4-07 podmienia moduł). Gdy ktoś poprosi o `diffuse` lub `specular`, pass koloru pisze trzy cele (`fs_split`); emisja idzie do `specular`, żeby SSS jej nie rozmywał, a suma `diffuse + specular` jest równa `hdr`. Obiekty przezroczyste (flaga materiału) idą osobnym passem po opaque, posortowane od najdalszych, z blendingiem premultiplied i testem głębi bez zapisu; to minimum, pełne M4-15 (transmisja, kopia koloru) zostaje na później.

Bez `IblEnvironment` paczka wiąże czarne środowisko 1×1, więc scena oświetlona samymi światłami rysuje się bez `IblPack`. Bez `TextureBinder` materiały nie mają tekstur (domyślny hook). Ścieżkę tekstur (bindless albo per-materiał) wybiera binder z `Caps`; paczka używa jednego numeru grupy (`TEXTURE_GROUP` = 1) i layoutu binderu.

### Dane klatki

Stałe widoku, tablica instancji (`model`, `previous_model`, macierz normalnych, indeks materiału; 160 B) i tablica świateł (64 B) trafiają przez CPU do jednego bufora roboczego, restartowanego z `record_epoch` i rosnącego przez podwajanie (stary bufor idzie do `release_later`). Minimalny rozmiar bindingu to 256 B, bo pusty storage buffer nie przechodzi walidacji wymaganego rozmiaru elementu. Wgrywanie przyrostowe z `Instances::changes()` jest otwarte (M4-11).

## Konsekwencje

- Zgodność: wszystkie moduły WESL składają się na `rhi-null` we wszystkich 48 permutacjach materiału, na obu ścieżkach tekstur (test `shaders`); obraz bindless i per-materiał jest identyczny (test GPU).
- Testy GPU (Vulkan i DX12, RX 9070 XT): klastry względem brute force (10–1000 świateł, perspektywa i ortografia, przepełnienie, punkty losowe), wszystkie sloty prepassu względem geometrii sceny (głębia, normalne, id materiału, ruch obiektu i kamery ±0,5 px, roughness i albedo), piramida względem referencji CPU, golden `pbr_scene`, brak NaN, `diffuse + specular = hdr`, przezroczysta tafla. Referencja na lavapipe, WARP i Metalu dopiero w CI.
- Ograniczenia znane: brak culling (rysowane są wszystkie widoczne instancje, jedno wywołanie na instancję), brak cullingu ścian (`CullMode::None`, podwójna strona przez `front_facing`), brak lustrzanych transformacji (znak stycznej), jedna pula geometrii o układzie `standard`/`skinned`, brak okluzji z bent normals, SSAO i cieni (M4-07, M5-02), brak AA specular geometrycznego (pochodne ekranowe nie istnieją w ścieżce compute, ADR-0030).
- Wydajność: nie zmierzona w tym zadaniu; benchmark GPU (scena PBR w 1440p) zostaje wpisem w M4-17.
