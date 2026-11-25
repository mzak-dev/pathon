# pathon-bindless

Tekstury materiałów na dwóch ścieżkach (M4-04, [ADR-0036](../adr/0036-bindless-i-bind-group-per-material.md)): jedna bind group z `binding_array` na urządzeniach, które to potrafią (profil Desktop), albo jedna bind group na materiał tam, gdzie nie (profil Web). Materiał, jego `MaterialId` i jego WGSL są te same; zmienia się tylko moduł `material::texture_source`, który ten crate generuje dla wybranej ścieżki.

To zwykła paczka: zależy tylko od `pathon-api` (`[package.metadata.pathon] kind = "pack"`). Nie zna `pathon-material` ani `pathon-texture`: tekstura to `TextureViewId`, a z materiałem łączy ją hook WGSL przekazany jako tekst. Nie ładuje obrazów (to `pathon-texture`) i nie rysuje.

```rust
# use pathon_bindless::{BinderDesc, SamplerPreset, TextureBinder, TexturePath};
# use pathon_rhi::{DeviceApi, Format, NullDevice, TextureDesc, TextureUsage, TextureViewDesc};
# fn main() -> Result<(), pathon_bindless::BindlessError> {
let device = NullDevice::new(); // profil High: ma tablice bindingów
let mut binder = TextureBinder::new(&device, &BinderDesc::default())?;
assert!(matches!(binder.path(), TexturePath::Bindless { capacity: 1024 }));

let texture = device.create_texture(&TextureDesc::new_2d("albedo", 4, 4, Format::Rgba8Unorm, TextureUsage::SAMPLED))?;
let view = device.create_texture_view(texture, &TextureViewDesc::default())?;
let handle = binder.register(view, SamplerPreset::LinearRepeat)?;
let slot = binder.shader_value(handle)?; // TextureSlot(slot) w StandardMaterial::textures

binder.prepare(&device)?; // jedna przebudowa grupy po paczce zmian
let group = binder.bind_group(&device, 0, &[handle])?; // set_bind_group(1, group)
# let _ = (slot, group);
# Ok(())
# }
```

## Użycie

1. `TextureBinder::new(device, &BinderDesc)` wybiera ścieżkę z `Caps` urządzenia ([niżej](#wybór-ścieżki)).
2. Tekstury (2D, próbkowane jako `f32`, w stanie `SAMPLED`, np. z `TextureLoader::load` i `Texture::create_view`) rejestruj przez `register(view, SamplerPreset)`; w zamian dostajesz `TextureHandle`. Wartość dla materiału to `binder.shader_value(handle)`, czyli `MaterialTextures { base_color: Some(TextureSlot(wartość)), .. }` w `pathon-material`. **Ta sama wartość na obu ścieżkach**, więc tablica materiałów jest identyczna.
3. Hook: `hooks.set_texture_source(binder.texture_source_wgsl(GRUPA))` w `MaterialHooks`, **przed** rejestracją modułów materiału w `ShaderLibrary` / `BackgroundCompiler`. `GRUPA` to numer grupy tekstur w pipeline'ie (w przykładach 1; grupa 0 to ramka, materiały i instancje).
4. Pipeline ma `binder.layout()` pod tym numerem. Layout zgadza się z tym, co zwraca `reflect_bind_groups` dla złożonego shadera (test `composition`).
5. Co klatka: `begin_frame(device, frame)` (zwalnia sloty i stare grupy), potem `register` / `unregister`, potem `prepare`. Przy każdym rysowaniu `pass.set_bind_group(GRUPA, binder.bind_group(device, material_id.index(), &uchwyty_tekstur_materiału)?)`: ten sam kod dla obu ścieżek.
6. Na koniec `release(device)` po `wait_idle`.

`bind_group` dostaje **wszystkie** tekstury, które materiał ma w danych (do `MAX_MATERIAL_TEXTURES` = 8), w dowolnej kolejności. Na ścieżce bindless zwraca zawsze tę samą wspólną grupę (liczy się tylko sprawdzenie uchwytów), na per-materiał grupę tego materiału.

## Wybór ścieżki

`choose_path(&Caps, PathPreference, capacity)` (czysta funkcja; `TextureBinder::new` woła ją z `device.caps()`):

- bindless: `Caps::features` ma `TEXTURE_BINDING_ARRAY | NON_UNIFORM_INDEXING` (`BINDLESS_FEATURES`) i `max_binding_array_elements_per_shader_stage` ≥ 64; długość tablicy to `min(capacity, limit)`;
- inaczej per-materiał.

| `PathPreference` | z tablicami | bez tablic |
| --- | --- | --- |
| `Auto` | bindless | per-materiał, `fallback_diagnostic()` = [B0001](../diag/B0001.md) |
| `Bindless` | bindless | błąd [B0002](../diag/B0002.md) |
| `PerMaterial` | per-materiał | per-materiał |

Poproś o cechy w `DeviceRequest` (`optional_features: BINDLESS_FEATURES` dla aplikacji, która ma działać wszędzie). Profile: Web nie ma żadnej z nich, Desktop i High mają obie ([`Profile::features`](pathon-rhi-types.md)).

## `TextureTable`

Rejestr tekstur z indeksem logicznym: `add`, `remove`, `view`, `shader_value`, `begin_frame`, `capacity`, `live_count`, `revision`. Używa go `TextureBinder`, a osobno nadaje się do własnych ścieżek (hit shadery, własny silnik).

- **Uchwyt generacyjny:** `TextureHandle` ma `index` (slot) i generację. `remove` unieważnia uchwyt od razu, slot wraca do puli po `frames_in_flight` klatkach. Stary uchwyt po ponownym użyciu slotu jest odrzucany ([B0004](../diag/B0004.md)).
- **Stała pojemność** (domyślnie 1024, max `MAX_CAPACITY` = 2²⁴), bo długość `binding_array` jest w skompilowanym shaderze. Pełna tablica: [B0003](../diag/B0003.md). Pojemność 0 i zero klatek w locie: [B0006](../diag/B0006.md).
- **Wartość shadera:** slot w bitach 0–23, `SamplerPreset` w bitach 24–25; `u32::MAX` zostaje „brak tekstury”. `decode_shader_value` rozdziela ją z powrotem.
- Tabela **nie jest właścicielem** widoków ani tekstur. `remove` oddaje `TextureViewId`: zniszcz go najwcześniej `frames_in_flight` klatek później (bind groupy w locie go jeszcze trzymają), np. przez `DeferredRelease`.

## Samplery

Cztery stałe presety (`SamplerPreset`: liniowy albo najbliższy, powtarzanie albo clamp), jako cztery zwykłe bindingi, nie `binding_array<sampler>`: limit sampler-ów w tablicy to zero na Web i mały na części urządzeń (`Limits::max_binding_array_sampler_elements_per_shader_stage`, [ADR-0017](../adr/0017-tablice-bindingow.md)). Preset jest właściwością tekstury (`register`), a shader wybiera go z bitów 24–25 wartości slotu. Bez anizotropii.

## Ścieżka bindless

Jedna grupa: binding 0 to `binding_array<texture_2d<f32>, N>` (`N` = pojemność), bindingi 1–4 to samplery. Bind groupy są niemutowalne, więc:

- `register` / `unregister` zmieniają tablicę (`revision`), nic nie budują;
- `prepare` buduje **jedną** nową grupę z `N` widokami, gdy `revision` się zmieniła; puste i zwolnione sloty wskazują placeholder (biała tekstura 1×1). Stara grupa jest niszczona po `frames_in_flight` klatkach (`DeferredRelease`). `group_builds()` liczy przebudowy;
- `bind_group` woła `prepare`, gdy grupa jest nieaktualna: zapomnienie `prepare` kosztuje przebudowę na zmianę, nie poprawność.

W WGSL: `enable wgpu_binding_array;` (WESL przenosi je na początek złożonego shadera), indeks `slot & 0xffffff`, `textureSampleGrad` z samplerem wybranym `switch`-em. Indeks jest nie-jednolity (inny dla każdego piksela), stąd `NON_UNIFORM_INDEXING`.

## Ścieżka per-materiał

Grupa na materiał: binding 0 to uniform `array<vec4<u32>, 2>` z wartościami slotów (osiem), bindingi 1–8 to tekstury, 9–12 to samplery. Moduł `texture_source` porównuje wartość slotu z uniformem i próbkuje odpowiedni binding; slot spoza zestawu i `NO_TEXTURE` dają biel. Dzięki temu `MaterialData` jest bajt w bajt taki sam jak na ścieżce bindless.

- `MAX_MATERIAL_TEXTURES` = 8: pięć map glTF i trzy miejsca na rozszerzenia. Więcej: [B0005](../diag/B0005.md) (na obu ścieżkach).
- Grupa powstaje przy pierwszym `bind_group(material, ...)` i gdy zestaw uchwytów się zmieni; stara (razem z buforem uniformu) idzie do odroczonego zwolnienia.
- `unregister(handle)` usuwa grupy, które go zawierały (kolejny `bind_group` dla tego uchwytu to `B0004`); `forget_material(id)` usuwa grupę usuniętego materiału.

## Diagnostyka

| Kod | Sytuacja |
| --- | --- |
| [B0001](../diag/B0001.md) | ostrzeżenie: brak tablic, tekstury idą przez bind group per materiał |
| [B0002](../diag/B0002.md) | bindless wymagany (`PathPreference::Bindless`), ale niedostępny |
| [B0003](../diag/B0003.md) | tablica tekstur pełna |
| [B0004](../diag/B0004.md) | uchwyt tekstury nieaktualny albo obcy |
| [B0005](../diag/B0005.md) | materiał ma więcej niż `MAX_MATERIAL_TEXTURES` tekstur |
| [B0006](../diag/B0006.md) | niepoprawna konfiguracja (pojemność, klatki w locie) |

Błędy urządzenia przechodzą jako `BindlessError::Rhi` z kodem `R…`. `BindlessError::to_diagnostic()` daje `Diagnostic` z podpowiedzią.

## Testy i przykład

- Jednostkowe: tabela `choose_path` dla `Caps` Web / Desktop / High / z jedną cechą / z małym limitem, generacje i ponowne użycie slotów w `TextureTable`.
- `tests/composition.rs` (`rhi-null`): źródło obu ścieżek składa się z materiałem standardowym, przechodzi walidację naga, a reflection daje dokładnie layout binderu; numer grupy jest wyborem aplikacji.
- `tests/binder.rs` (`rhi-null`, którego walidacja liczby elementów tablicy odpowiada wgpu): jedna przebudowa grupy na paczkę zmian, wypełnienie tablicy placeholderem do pojemności, odroczone niszczenie, pełna tablica, stare uchwyty, cache i unieważnianie grup per materiał, `B0001`…`B0006`, te same wartości shadera na obu ścieżkach.
- `tests/same_scene.rs`: podłoga, dwie kule i sześcian z [pathon-mesh](pathon-mesh.md), trzy materiały z sześcioma teksturami z [pathon-texture](pathon-texture.md) (kolor, normalna, metallic-roughness, okluzja, emisja, mipmapy z compute) i shader z [pathon-material](pathon-material.md). Na `rhi-null` zawsze (obie ścieżki, te same bajty tablicy materiałów); z `--features gpu-tests` na dwóch urządzeniach z jednego adaptera (z cechami tablicowymi i bez nich, czyli Web w `Caps`): obrazy zgadzają się co do piksela (±1) i z jedną referencją `tests/golden/bindless_scene.png` ([pathon-golden](pathon-golden.md)); drugi test wymusza per-materiał na urządzeniu z tablicami.
- `tests/dynamic.rs` (`--features gpu-tests`, obie ścieżki): tekstury dochodzą i znikają w trakcie, slot wraca po `frames_in_flight` klatkach, grupa jest przebudowana, odczyt pikseli.

```sh
cargo test -p pathon-bindless                                       # bez GPU
cargo test -p pathon-bindless --features gpu-tests -- --test-threads=1   # PowerShell: $env:PATHON_TEST_BACKEND="dx12"
cargo run -p pathon-examples --bin bindless_scene -- [--bindless | --per-material] [plik.png]
```

Przykład `bindless_scene` ([przykłady](../examples.md)): ta sama scena co w teście; bez flagi ścieżkę wybiera urządzenie, flagi ją wymuszają, a oba obrazy są identyczne. Urządzenie bez cech tablicowych spada na per-materiał z ostrzeżeniem `B0001`.
