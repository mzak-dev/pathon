# Kontrakt slotów

Sloty to nazwane tekstury, przez które paczki wymieniają dane w obrębie widoku. Format każdego slotu jest stały (`Slot::format()`); paczka, która zapisze slot innym formatem, dostaje G0005. Adaptery hostów (Bevy, Godot) konwertują bufory hosta do tego kontraktu w passach adaptera, nie w paczkach.

## Konwencje

- **Przestrzeń:** układ prawoskrętny, Y w górę, metry.
- **Głębia:** reverse-Z — bliżej = większa wartość, czyszczenie do `0.0`, test `GreaterEqual`.
- **Wektory ruchu:** w pikselach, z bieżącej klatki do poprzedniej, bez jittera.
- **Normalne:** w przestrzeni świata, kodowane oktaedralnie do dwóch kanałów.
- **Światło:** jednostki fizyczne (lux, lumen); `hdr` jest liniowe i pre-exposed, mnożnik w slocie `exposure`.

## Sloty

| Slot | Format | Rozmiar | Na żądanie | Zawartość |
| --- | --- | --- | --- | --- |
| `Slot::DEPTH` | `Depth32Float` | render | nie | głębia reverse-Z |
| `Slot::DEPTH_PYRAMID` | `R32Float` + mipy | render | tak | Hi-Z (min głębi) dla SSR i cullingu okluzji |
| `Slot::NORMALS` | `Rg16Float` | render | nie | normalne świata, oct-encoded |
| `Slot::MOTION` | `Rg16Float` | render | nie | ruch w pikselach do poprzedniej klatki |
| `Slot::MATERIAL_ID` | `R32Uint` | render | nie | indeks materiału w tablicy materiałów |
| `Slot::ROUGHNESS` | `R8Unorm` | render | tak | chropowatość perceptualna (SSR, DLSS RR) |
| `Slot::ALBEDO` | `Rgba8Unorm` | render | tak | albedo liniowe |
| `Slot::REACTIVE` | `R8Unorm` | render | tak | maska reaktywności dla upscalerów |
| `Slot::DIFFUSE` | `Rgba16Float` | render | nie | światło rozproszone (podział dla SSS) |
| `Slot::SPECULAR` | `Rgba16Float` | render | nie | światło odbite |
| `Slot::HDR` | `Rgba16Float` | render | nie | kolor sceny, liniowy, pre-exposed |
| `Slot::AO` | `R8Unorm` | render | nie | ambient occlusion |
| `Slot::EXPOSURE` | `R32Float` 1×1 | stały | nie | mnożnik ekspozycji |
| `Slot::OUTPUT` | format celu | wyjście | nie | obraz końcowy celu renderu |

Rozmiar „render” to `ViewCtx::render_size` (ustala rola TemporalResolve), „wyjście” to `ViewCtx::output_size`. Render pass nie może mieszać tych klas (G0004).

`Slot::OUTPUT` okna ma kodowanie przestrzeni barw powierzchni: `ViewCtx::output` (`OutputInfo`) mówi, jaka to przestrzeń (`Srgb` albo HDR, nigdy `Auto`) i jaki jest szczyt wyświetlacza w nitach ([ADR-0034](adr/0034-wyjscie-hdr.md)). Cele offscreen i importowane są `Srgb`. Tonemapping, który zamienia liniowy `hdr` na to kodowanie, robi `pathon-post` (M4-09).

## Własne sloty paczek

Lista powyżej to sloty wbudowane, nie zamknięty zbiór. Paczka może zadeklarować własny slot dla innych paczek:

```rust
pub const MASK: Slot = Slot::new("outline.mask", Format::R8Unorm);
pub const MOTION_HIRES: Slot = Slot::new("my_taa.motion_hires", Format::Rg16Float).on_demand();
```

Sloty są równe, gdy mają tę samą nazwę — prefiks paczki (`outline.`) chroni przed kolizją. Format własnego slotu jest jego kontraktem tak samo jak wbudowanego (G0005), a slot, którego nikt nie czyta, znika z grafu razem ze swoim producentem.
