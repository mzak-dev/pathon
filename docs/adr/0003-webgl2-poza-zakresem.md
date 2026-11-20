# ADR-0003: WebGL2 poza zakresem — web tylko przez WebGPU

**Status:** Przyjęta
**Data:** 2026-11-20

## Kontekst

wgpu ma backend GL, który w przeglądarce działa na WebGL2. Na koniec 2026 WebGPU jest dostępne w Chrome, Edge, Safari 26 i Firefoksie.

WebGL2 nie ma compute shaderów, storage bufferów ani rysowania indirect. Do tego wgpu na WebGL2 nie utworzy adaptera bez okna (canvasu).

## Decyzja

Web obsługujemy wyłącznie przez WebGPU. Przeglądarka bez WebGPU dostaje czytelny komunikat w stronie (M0-06) zamiast fallbacku.

## Rozważane opcje

| Opcja | Za | Przeciw |
| --- | --- | --- |
| **Tylko WebGPU** | jeden model danych w całym rdzeniu: compute, storage buffery i indirect są wszędzie | starsze przeglądarki i część urządzeń mobilnych odpada |
| WebGPU + fallback WebGL2 | szerszy zasięg | drugi model danych w rdzeniu (instancje i światła w uniformach i teksturach zamiast `GpuArray`, clustered forward bez compute, culling bez indirect); paczki potrzebują dwóch ścieżek; renderer headless na webie niemożliwy |

## Konsekwencje

- `GpuArray<T>` (M2-09), przypisanie świateł w compute (M4-05) i culling na GPU (M4-11) działają na każdym profilu bez wariantów.
- Godot na webie używa renderera Compatibility bez `RenderingDevice`, więc pathon w Godocie na webie i tak jest niemożliwy (poza zakresem). Bevy na webie z WebGL2 również nie jest wspierany przez pathon.
- Do rewizji, jeśli realny projekt będzie wymagał przeglądarek bez WebGPU — wtedy jako osobny, uproszczony renderer, nie gałąź w rdzeniu.
