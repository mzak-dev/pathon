# pathon-api

Kontrakt paczki (M2-10). Zwykła paczka zależy tylko od tego crate'u: re-eksportuje RHI (`rhi`), graf (`graph`), narzędzia pamięci (`resources`) i `glam`.

## `RenderFeature`

| Metoda | Kiedy | Po co |
| --- | --- | --- |
| `requirements()` | przed utworzeniem urządzenia | backend, wymagane i opcjonalne funkcje, rozszerzenia backendu (faza 1) |
| `negotiate(&Caps)` | po utworzeniu urządzenia | `Full`, `Fallback { reason }` (F0103) albo `Disabled { reason }` (F0101) (faza 2) |
| `setup(&mut SetupCtx)` | raz, po negocjacji | pipeline'y, bufory |
| `prepare_view(&mut PrepareCtx)` | co klatkę, przed budową grafu | prośby o sloty na żądanie; rola `TemporalResolve` ustawia `render_size`, jitter i mip bias |
| `record(&mut RecordCtx)` | co klatkę, dla każdego widoku | passy w grafie, odczyt i zapis slotów |
| `on_resize`, `on_device_reset` | przy zmianie rozmiaru / po utracie urządzenia | domyślnie `on_device_reset` woła `setup` |

Kolejność paczek: `stage()`, potem `before()` / `after()` (nazwy paczek), potem `priority()`; remis rozstrzyga rejestracja z G0006. Ten sam algorytm co dla passów (`sort_nodes`), więc paczki zmieniające ten sam slot dają kolejność niezależną od rejestracji.

`role()` — jedna aktywna paczka na rolę; druga dostaje F0102.

## Sloty

`Slot` i jego `format()` to kontrakt opisany w [docs/slots.md](../slots.md). `RecordCtx::set_slot` odrzuca teksturę w innym formacie (G0005); `create_slot_texture` tworzy ją od razu we właściwym. Sloty `is_on_demand()` powstają tylko, gdy konsument poprosi o nie w `prepare_view`.

## `ViewCtx`

Rozmiar wyjścia i renderu, jitter, mip bias, kamera (`Camera`, reverse-Z) oraz czas i numer klatki **od hosta** — core nie czyta zegara.

## `TaskPool`

ADR-0008: `parallel_for(len, &task)` blokuje do końca, więc zadanie może pożyczać dane klatki. `SerialTaskPool` jest domyślny (także na wasm).
