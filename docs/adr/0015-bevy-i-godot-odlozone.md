# ADR-0015: Bevy i Godot odłożone — najpierw dobry i stabilny renderer

**Status:** Przyjęta
**Data:** 2026-11-21

## Kontekst

Plan zakładał integrację z Bevy (M6) i Godotem (M7), a spike ash (M2-15) mapował RHI na `RenderingDevice` Godota. Część decyzji i zadań przed M3 (np. kopie bufor↔tekstura, importy powierzchni, offsety rysowania pod Godota) wynikała tylko z tych hostów.

## Decyzja

- **M6 (Bevy) i M7 (Godot) wypadają z zakresu do odwołania.** Celem jest renderer dobry i stabilny sam w sobie: rdzeń, paczki, jakość obrazu i wydajność.
- Generyczność rdzenia zostaje: rdzeń nadal nie ma pętli, okna ani wątku (ADR-0004), przyjmuje cudze urządzenie i tekstury (tryby A/B z ADR-0005) — to służy własnemu silnikowi i testom, nie konkretnemu hostowi.
- Zmiany w RHI uzasadnione wyłącznie Godotem lub Bevy nie wchodzą do planu; zmiany potrzebne natywnemu backendowi (M9) i paczkom zostają.

## Konsekwencje

- Wersja wgpu nie musi podążać za Bevy.
- Mapowanie na Godota w `docs/spike-ash.md` zostaje jako materiał historyczny.
- Powrót do Bevy lub Godota wymaga nowego ADR, który przywróci M6 / M7.
