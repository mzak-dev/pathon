# ADR-0006: Wejście shaderów do RHI

**Status:** Przyjęta
**Data:** 2026-11-20

## Kontekst

RHI ma dziś jeden backend (wgpu), później może dostać ash, Metal i Godot `RenderingDevice` (SPIR-V). Shadery piszą paczki, a refleksja ma budować layouty bind groupów (M3-02). WebGPU przyjmuje tylko WGSL.

`pathon-rhi-types` zależy tylko od `bitflags` i `raw-window-handle` (M0-07).

## Decyzja

- RHI przyjmuje **źródło WGSL** (już złożone z importów przez `pathon-shader`, M3-01) plus **refleksję w typach pathon** (opisy layoutów bind groupów, wejść wierzchołków, wyjść). Refleksję liczy `pathon-shader` z naga.
- Backend wgpu podaje WGSL do wgpu. Backendy natywne (ash, Metal, Godot) tłumaczą WGSL przez naga na SPIR-V lub MSL **we własnym crate'cie** i keszują wynik.
- Paczki piszą wyłącznie WGSL. Funkcje spoza standardu (ray query, 64-bit atomics) przez rozszerzenia WGSL obsługiwane przez naga, zgłoszone w `requirements()`.

## Rozważane opcje

| Opcja | Za | Przeciw |
| --- | --- | --- |
| **WGSL + refleksja w typach pathon** | jedyny format, który działa wszędzie, także na webie; API RHI nie zależy od wersji naga | WGSL parsowany dwa razy (refleksja i backend); koszt znika w keszu |
| `naga::Module` w API RHI | jedno parsowanie | publiczne API zależne od wersji naga, która zmienia się z każdą główną wersją wgpu; `rhi-types` ciągnie naga |
| SPIR-V jako wejście | standard natywnych API | WebGPU go nie przyjmuje; paczki potrzebowałyby kompilatora |
| HLSL / Slang | dojrzałe narzędzia, łatwe porty z innych silników | dodatkowy toolchain (DXC / slangc) w buildzie; i tak potrzebne tłumaczenie do WGSL na web |

## Konsekwencje

- Możliwości shaderów kończą się na tym, co obsługuje frontend WGSL w naga (np. mesh shadery w WGSL są eksperymentalne, M8-02 ich nie wymaga).
- Porty shaderów z HLSL (FSR 1 i inne) trzeba ręcznie przepisać na WGSL.
- Błędy shaderów (M3-07) raportuje jeden frontend: naga.
