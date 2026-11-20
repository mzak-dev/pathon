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
| `Depth` | `Depth32Float` | render | nie | głębia reverse-Z |
| `DepthPyramid` | `R32Float` + mipy | render | tak | Hi-Z (min głębi) dla SSR i cullingu okluzji |
| `Normals` | `Rg16Float` | render | nie | normalne świata, oct-encoded |
| `Motion` | `Rg16Float` | render | nie | ruch w pikselach do poprzedniej klatki |
| `MaterialId` | `R32Uint` | render | nie | indeks materiału w tablicy materiałów |
| `Roughness` | `R8Unorm` | render | tak | chropowatość perceptualna (SSR, DLSS RR) |
| `Albedo` | `Rgba8Unorm` | render | tak | albedo liniowe |
| `Reactive` | `R8Unorm` | render | tak | maska reaktywności dla upscalerów |
| `Diffuse` | `Rgba16Float` | render | nie | światło rozproszone (podział dla SSS) |
| `Specular` | `Rgba16Float` | render | nie | światło odbite |
| `Hdr` | `Rgba16Float` | render | nie | kolor sceny, liniowy, pre-exposed |
| `Ao` | `R8Unorm` | render | nie | ambient occlusion |
| `Exposure` | `R32Float` 1×1 | stały | nie | mnożnik ekspozycji |
| `Output` | format celu | wyjście | nie | obraz końcowy celu renderu |

Rozmiar „render” to `ViewCtx::render_size` (ustala rola TemporalResolve), „wyjście” to `ViewCtx::output_size`. Render pass nie może mieszać tych klas (G0004).
