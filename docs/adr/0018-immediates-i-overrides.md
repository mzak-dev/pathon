# ADR-0018: Immediates i overrides

**Status:** Przyjęta
**Data:** 2026-11-21

## Kontekst

Shadery potrzebują dwóch rodzajów małych, szybko zmiennych danych: wartości per draw (indeks materiału, transformacja) i stałych wybierających wariant shadera (permutacje M3-03). wgpu 30 nazywa pierwsze **immediates** (`Features::IMMEDIATES`, `Limits::max_immediate_size`, `set_immediates(offset, data)` bez etapu, WGSL `var<immediate>`), a drugie **pipeline overrides** (`constants: &[(&str, f64)]` w opcjach kompilacji). Spike ash (`docs/spike-ash.md`, pozycja #5) opisał je jeszcze jako push constants z etapem, w kształcie sprzed wgpu 28.

## Decyzja

- **Immediates:** `Features::IMMEDIATES`, `Limits::max_immediate_size`; `RenderPipelineDesc` i `ComputePipelineDesc` mają `immediate_size: u32` (0 = brak). `RenderPassEncoder` / `ComputePassEncoder` dostają `set_immediates(offset, data)`; komenda `Command::SetImmediates` mieści się w limicie 32 bajtów (ADR-0009) — dane w `Box<[u8]>`. Jeden blok widoczny dla wszystkich etapów (bez maski etapów), bo tak robi wgpu.
- **Overrides:** `ShaderStage::constants: Vec<(&'static str, f64)>` — nazwane stałe `override` z WGSL, przekazywane do `PipelineCompilationOptions::constants`. Natywny backend (Vulkan) rozwiąże je w naga przed generowaniem SPIR-V.
- **Reguły w `rhi-null`:** `immediate_size > 0` wymaga `Features::IMMEDIATES` i `≤ max_immediate_size`; `set_immediates` tylko w passie z ustawionym potokiem, `offset` i długość wyrównane do 4 bajtów, `offset + len ≤ immediate_size` potoku.
- **Web:** przeglądarki nie mają jeszcze immediates; profil Web nie ma `IMMEDIATES`, więc paczki potrzebują ścieżki przez uniform buffer.

## Konsekwencje

- `ShaderStage` traci `Copy`; nowe pola w literałach opisów potoków — zmiana łamiąca przed M5-07.
- Klucz potoku w M3-03 obejmuje `constants` i `immediate_size`.
