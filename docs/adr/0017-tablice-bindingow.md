# ADR-0017: Tablice bindingów (bindless) w RHI

**Status:** Przyjęta
**Data:** 2026-11-21

## Kontekst

Bindless (M4-04: materiały jako indeksy do tablicy tekstur) wymaga tablic bindingów. `BindGroupLayoutEntry` nie miał liczby elementów, a `BindingResource` — zasobu tablicowego; spike ash (`docs/spike-ash.md`, pozycja #1) musiał zbudować layout z 1024 teksturami bezpośrednio w ash. `Features::TEXTURE_BINDING_ARRAY`, `NON_UNIFORM_INDEXING` i `PARTIALLY_BOUND_BINDING_ARRAY` istnieją od M1-01, ale nic ich nie używało.

## Decyzja

- **Layout:** `BindGroupLayoutEntry::count: Option<NonZeroU32>`; `Some(n)` to stała tablica `n` elementów (jak `binding_array<T, N>` z naga). Bez zmiennej liczby (`VARIABLE_DESCRIPTOR_COUNT`) i bez reguły „tablica ostatnia”.
- **Zasoby:** `BindingResource::TextureViewArray(Vec<TextureViewId>)` i `BindingResource::SamplerArray(Vec<SamplerId>)`. Tablice buforów dojdą, gdy będzie ich użytkownik. `BindingResource` i `BindGroupEntry` tracą `Copy`.
- **Model aktualizacji:** bind groupy są niemutowalne; zmiana elementu tablicy = utworzenie nowej grupy (tak działa wgpu). Bez publicznej flagi update-after-bind i bez API aktualizacji pojedynczego elementu.
- **Reguły (walidowane w `rhi-null`, kod R0107):** layout z `count` wymaga `Features::TEXTURE_BINDING_ARRAY` i mieści się w `Limits::max_binding_array_elements_per_shader_stage` (dla samplerów — w nowym `Limits::max_binding_array_sampler_elements_per_shader_stage`); grupa z tablicą nie zawiera uniform buffera (reguła wgpu `ContainsBothBindingArrayAndUniformBuffer`); liczba zasobów w tablicy równa się `count`, chyba że włączone jest `PARTIALLY_BOUND_BINDING_ARRAY` (wtedy ≤ `count`); zasób tablicowy tylko dla wpisu z `count` i odwrotnie.
- **Zakres `rhi-null`:** odrzuca też tablice buforów i tekstur storage (R0107) — wgpu wymagałby `BUFFER_BINDING_ARRAY` / `STORAGE_RESOURCE_BINDING_ARRAY`, których pathon nie żąda; limity liczy się na etap (suma elementów wszystkich wpisów widocznych dla etapu, samplery także w limicie ogólnym); tablica ma co najmniej jeden element.
- **Poza zakresem tego tasku:** `NON_UNIFORM_INDEXING` nie jest jeszcze ćwiczone testem GPU — do zrobienia przed M4-04.
- **Web:** profil Web nie ma `TEXTURE_BINDING_ARRAY`; paczki potrzebują ścieżki bez tablic (bind group per materiał, M4-04).

## Konsekwencje

- Zmiana łamiąca dla paczek (nowe pole w literałach `BindGroupLayoutEntry`, utrata `Copy`); przed M5-07 to tanie.
- Odtwarzanie grupy z 1024 elementami przy zmianie materiału kosztuje; jeśli pomiar w M4-17 to pokaże, wrócimy do API aktualizacji z fallbackiem na wgpu.
