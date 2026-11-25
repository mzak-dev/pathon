# ADR-0036: Bindless tekstur i bind group per materiał

**Status:** Przyjęta
**Data:** 2026-11-22

## Kontekst

Standardowy materiał ([ADR-0030](0030-material-standardowy-i-hooki-wgsl.md)) czyta tekstury w jednym miejscu, `sample_material_texture(slot, uv, uv_dx, uv_dy)`, i trzyma w danych tylko **logiczny slot**. M4-04 ma sprawić, że ten sam materiał, z tym samym `MaterialId` i tymi samymi danymi, działa na dwóch urządzeniach: profil Desktop ma tablice bindingów ([ADR-0017](0017-tablice-bindingow.md), `TEXTURE_BINDING_ARRAY` + `NON_UNIFORM_INDEXING`, test GPU z PAT-119), profil Web (WebGPU) ich nie ma. Do rozstrzygnięcia: gdzie to mieszka, jak wybrać ścieżkę, jak aktualizować niemutowalne bind groupy, co z samplerami i ile tekstur ma materiał na ścieżce bez tablic.

## Decyzja

### Osobny crate `pathon-bindless`

Paczka (`kind = "pack"`), zależy tylko od `pathon-api` (reguła `pack-boundary`, [ADR-0023](0023-permutacje-shaderow.md)). **Nie** siedzi w `pathon-material`, bo:

- materiał opisuje powierzchnię (WGSL, dane, permutacje), a bindless jest zagadnieniem bind groupów i zasobów; po zmianie jednego nie trzeba rekompilować ani przeglądać drugiego;
- jedyną wspólną powierzchnią jest hook `material::texture_source` z ADR-0030, czyli **tekst WGSL**: `TextureBinder::texture_source_wgsl(group)` oddaje źródło, a `MaterialHooks::set_texture_source` je przyjmuje. Żaden z crate'ów nie zna typów drugiego, więc paczka (`pathon-pbr`, M4-05) składa je przez `Shared`, a testy i przykłady mogą zależeć od obu;
- `TextureTable` przechowuje `TextureViewId`, nie `pathon_texture::Texture`: nie zależy też od `pathon-texture`.

### Wybór ścieżki z `Caps`, nie z nazwy profilu

`choose_path(&Caps, PathPreference, capacity)` to czysta funkcja (testowana tabelarycznie na zmyślonych `Caps`). Ścieżka bindless wymaga `Features::TEXTURE_BINDING_ARRAY | NON_UNIFORM_INDEXING` i `Limits::max_binding_array_elements_per_shader_stage ≥ MIN_BINDLESS_CAPACITY` (64); pojemność jest przycinana do limitu urządzenia (shader powstaje dla ostatecznej długości tablicy). Profil Desktop i High spełniają to z definicji; urządzenie, które ma tylko jedną z cech, spada na per-materiał.

| `PathPreference` | Urządzenie ma tablice | Urządzenie nie ma |
| --- | --- | --- |
| `Auto` (domyślnie) | bindless | per-materiał + ostrzeżenie `B0001` (z brakującymi cechami) |
| `Bindless` | bindless | błąd `B0002` z podpowiedzią (`DeviceRequest`) |
| `PerMaterial` | per-materiał | per-materiał |

`PerMaterial` służy testom i przykładom (flaga `--per-material`): ta sama ścieżka co Web, na urządzeniu, które mogłoby więcej.

### `TextureTable`: slot generacyjny, stała pojemność, placeholder

- `TextureHandle { index, generation }`; `add` zwraca uchwyt, `remove` unieważnia go **od razu** (generacja rośnie), a sam slot wraca do puli dopiero po `frames_in_flight` klatkach (`begin_frame`, [ADR-0022](0022-zegar-klatki-paczek.md)), bo lista nagrana, a niezgłoszona, może go jeszcze czytać. Stary uchwyt jest odrzucany (`B0004`) nawet po ponownym użyciu slotu.
- **Pojemność jest stała** (domyślnie `DEFAULT_CAPACITY` = 1024, górna granica 2²⁴): długość `binding_array` wchodzi do skompilowanego shadera. Pełna tablica to `B0003`; rośnięcie to nowy binder, nowe źródło WGSL i nowe pipeline'y, nie operacja w locie.
- Puste i zwolnione sloty wskazują **placeholder**: biała tekstura 1×1 `Rgba8Unorm`. Dzięki temu grupa zawsze ma dokładnie `capacity` elementów i nie wymaga `PARTIALLY_BOUND_BINDING_ARRAY` (które nie jest w każdym profilu i w `rhi-null` zmienia regułę liczby elementów).
- Wartość w danych materiału (`TextureTable::shader_value`) to slot w dolnych 24 bitach i `SamplerPreset` w bitach 24–25. `u32::MAX` (`NO_TEXTURE`) pozostaje „brak tekstury”, bo slot < 2²⁴ − 1 nigdy nie ma wszystkich 24 bitów. **Ta sama wartość trafia do `MaterialData` na obu ścieżkach**: test porównuje bajty tablicy materiałów.

### Samplery: cztery stałe, osobne bindingi

Zamiast `binding_array<sampler>` (limit `max_binding_array_sampler_elements_per_shader_stage` jest zerem na profilu Web i niewielki na części urządzeń) binder tworzy cztery sampler-y (`SamplerPreset`: liniowy / najbliższy × powtarzanie / clamp) jako cztery zwykłe bindingi, a shader wybiera jeden przez `switch` na bitach 24–25 wartości slotu (`textureSampleGrad` nie wymaga jednolitego przepływu). Bez anizotropii: ten sam obraz na każdym GPU i na obu ścieżkach. Zestaw presetów jest zamknięty: kolejny sampler (np. `MirrorRepeat`) to dopisanie na końcu `SamplerPreset::ALL` i jednego bindingu.

### Model aktualizacji (ADR-0017 w praktyce)

Bind groupy są niemutowalne, więc zmiana elementu tablicy to nowa grupa.

- **Bindless:** `register` / `unregister` tylko zmieniają tablicę i podbijają `revision`. `TextureBinder::prepare` (raz na klatkę, po paczce zmian) buduje **jedną** nową grupę z `capacity` widokami, jeśli `revision` się zmieniła; stara idzie do `DeferredRelease` i jest niszczona po `frames_in_flight` klatkach. `bind_group` woła `prepare` samo, gdy grupa jest nieaktualna, więc zapomnienie go kosztuje przebudowę na zmianę, nie poprawność. Koszt przebudowy to O(`capacity`) po stronie CPU; jeśli pomiar w M4-17 pokaże, że 1024 elementów na klatkę boli, wracamy do update-after-bind z ADR-0017.
- **Per-materiał:** `bind_group(material, &[uchwyty])` buduje grupę materiału przy pierwszym użyciu i gdy zestaw uchwytów się zmieni; `unregister` usuwa grupy zawierające dany uchwyt (zwolnienie odroczone), `forget_material` — grupę usuniętego materiału.

### Ścieżka per-material: zestaw tekstur materiału

Grupa ma 13 bindingów: uniform ze slotami (`array<vec4<u32>, 2>` = 8 wartości), **8 tekstur** (`MAX_MATERIAL_TEXTURES`), 4 samplery. Osiem to pięć map glTF z ADR-0030 plus miejsce na mapy rozszerzeń (clearcoat, transmisja, specular) bez zmiany układu, przy jednocześnie bezpiecznym limicie WebGPU (16 próbkowanych tekstur i 16 sampler-ów na etap). Więcej to `B0005`; ten sam limit obowiązuje ścieżkę bindless, żeby materiał nie działał tylko na jednej z nich.

Moduł `texture_source` tej ścieżki **porównuje wartość slotu z uniformem** i próbkuje odpowiedni binding (do ośmiu porównań wartości jednolitych na próbkę). Dzięki temu dane materiału i jego WGSL nie wiedzą, którą ścieżką idą: slot w `MaterialData` to zawsze slot tablicy. Slot spoza zestawu materiału daje biel (jak „brak tekstury”); wołający podaje do `bind_group` **wszystkie** tekstury, które materiał trzyma.

### Hook: `texture_source` generowany per ścieżka

`texture_source_wgsl(group)` zwraca moduł WESL, który podmienia domyślny (`vec4(1.0)`): na ścieżce bindless deklaruje `binding_array<texture_2d<f32>, N>` w `group` i indeksuje go nie-jednolicie, na per-materiał deklaruje opisane wyżej bindingi. To **wyjątek od zasady z ADR-0030**, że moduły materiału nie deklarują bindingów: hook tekstur musi je mieć, a numer grupy wybiera aplikacja (`reflect_bind_groups` zwraca układ, test porównuje go z `TextureBinder::layout_desc`). `enable wgpu_binding_array;` w module przechodzi przez WESL na początek złożonego shadera (test `composition`).

Grupa z tablicą nie zawiera uniform buffera (reguła wgpu z ADR-0017), dlatego uniform ze slotami istnieje tylko na ścieżce per-materiał.

### Poprawka w `pathon-rhi-wgpu`

Widok sRGB nad teksturą `Rgba8Unorm` (ADR-0031: tak `pathon-texture` trzyma kolor) dziedziczył `STORAGE_BINDING` z tekstury, do której compute pisze mipy, i wgpu odrzucał go walidacją. `create_texture_view` zawęża teraz `usage` widoku o `STORAGE_BINDING`, gdy widok zmienia format. Bez tego żadna tekstura sRGB z mipami z GPU nie dałaby się użyć w materiale.

## Konsekwencje

- **Test „ta sama scena”** (`crates/pathon-bindless/tests/same_scene.rs`, golden `bindless_scene`): podłoga, dwie kule i sześcian z `pathon-mesh`, trzy materiały z sześcioma teksturami z `pathon-texture` (kolor, normalna, metallic-roughness, okluzja, emisja; mipmapy z compute). Raz ścieżką bindless, raz per-materiał, na dwóch urządzeniach z jednego adaptera: pierwsze z `TEXTURE_BINDING_ARRAY | NON_UNIFORM_INDEXING`, drugie bez żadnej cechy (Web w `Caps`). Obrazy zgadzają się co do piksela (próg ±1, w praktyce bit w bit; Vulkan i DX12, RX 9070 XT) i z jedną referencją (`cross_backend`). Ta sama scena przechodzi na `rhi-null` na obu profilach z walidacją wszystkich komend.
- Test `dynamic.rs` (GPU, obie ścieżki) sprawdza życie tekstur w locie: slot zwolniony jest ponownie użyty po `frames_in_flight` klatkach, grupa przebudowana, sąsiednie tekstury nietknięte, stary uchwyt martwy.
- Test `binder.rs` (`rhi-null`): pojemność i wypełnienie tablicy (null egzekwuje liczbę elementów jak wgpu), jedna przebudowa na paczkę zmian, odroczone niszczenie starych grup, pełna tablica, `B0001`…`B0006`.
- **Nie zweryfikowano w przeglądarce:** ścieżka per-materiał jest sprawdzona na natywnym wgpu z urządzeniem bez cech tablicowych, nie w Chrome. Widok tekstury 2D `f32` i `textureSampleGrad` są w podstawowym WebGPU, więc ryzyko jest małe, ale M4-10 ma ją potwierdzić.
- Widoczność bindingów to domyślnie `FRAGMENT | COMPUTE` (`BinderDesc::visibility`); vertex stage i hit shadery RT nie próbkują tekstur materiału w tym zadaniu.
- **Otwarte:** `pathon-pbr` (M4-05) jest paczką, a `pack-boundary` pozwala jej zależeć tylko od `pathon-api` i `pathon-shader`, więc nie może wprost używać `MaterialHooks` ani `TextureBinder`. Hook jest tekstem, ale same typy muszą się spotkać w kompozycji (host, przykłady, testy) albo w poluzowanej regule; to dotyczy już `pathon-material` z ADR-0030 i rozstrzygnie się przy M4-05.
- Zmiana tekstury w istniejącym slocie nie istnieje (`remove` + `add` daje nowy uchwyt); materiał trzeba wtedy zaktualizować (`MaterialTable::set`).
