# pathon-api

Kontrakt paczki (M2-10) — wszystko, czym paczka rozszerza silnik. Zwykła paczka zależy tylko od tego crate'u: re-eksportuje RHI (`rhi`), graf (`graph`), narzędzia pamięci (`resources`) i `glam`.

## Punkty rozszerzenia

| Co | Jak | Przykład |
| --- | --- | --- |
| Pass w klatce | `RenderFeature::record` dodaje passy do grafu | SSAO, bloom, tonemapping |
| Tekstura dla innych paczek | slot: wbudowany (`Slot::HDR`…) albo własny `Slot::new("pack.name", format)` | maska obrysu, wektory ruchu wysokiej rozdzielczości |
| Dane dla innych paczek | `Shared` — wartości kluczowane typem, w `SetupCtx`, `PrepareCtx`, `RecordCtx` | bufory sceny, lista świateł, tablica materiałów, ustawienia |
| Wyłączność | rola: wbudowana (`Role::GEOMETRY`…) albo własna `Role::new("sky")` | geometria rasteryzowana vs meshlety, TAA vs FSR |
| Kilka klas naraz | `Pack::register` — paczka instaluje kilka `RenderFeature` i ich wspólne dane | PBR: prepass + oświetlenie + IBL |
| Funkcje GPU | `requirements()` i `negotiate(&Caps)` | ray query z fallbackiem do cieni rastrowych |

Każdy pojedynczy `RenderFeature` jest też `Pack` (implementacja ogólna), więc `with_pack(Tint)` i `with_feature(Tint)` robią to samo.

## `RenderFeature`

| Metoda | Kiedy | Po co |
| --- | --- | --- |
| `requirements()` | przed utworzeniem urządzenia | backend, wymagane i opcjonalne funkcje, rozszerzenia backendu (faza 1) |
| `negotiate(&Caps)` | po utworzeniu urządzenia | `Full`, `Fallback { reason }` (F0103) albo `Disabled { reason }` (F0101) (faza 2) |
| `setup(&mut SetupCtx)` | raz, po negocjacji | pipeline'y, bufory, publikacja danych w `Shared` |
| `prepare_view(&mut PrepareCtx)` | co klatkę, przed budową grafu | prośby o sloty na żądanie; rola `TEMPORAL_RESOLVE` ustawia `render_size`, jitter i mip bias |
| `record(&mut RecordCtx)` | co klatkę, dla każdego widoku | passy w grafie, odczyt i zapis slotów i `Shared` |
| `on_resize`, `on_device_reset` | przy zmianie rozmiaru / po utracie urządzenia | domyślnie `on_device_reset` woła `setup` |

Kolejność paczek: `stage()`, potem `before()` / `after()` (nazwy paczek), potem `priority()`; remis rozstrzyga rejestracja z G0006. Ten sam algorytm co dla passów (`sort_nodes`), więc paczki zmieniające ten sam slot dają kolejność niezależną od rejestracji.

`role()` — jedna aktywna paczka na rolę; druga dostaje F0102. `Role::TEMPORAL_RESOLVE` to jedyna rola, którą rdzeń traktuje specjalnie: jej posiadacz ustala rozdzielczość renderu.

## Sloty

`Slot` ma nazwę, format i flagę „na żądanie”; wbudowane sloty i ich konwencje opisuje [docs/slots.md](../slots.md). `RecordCtx::set_slot` odrzuca teksturę w innym formacie (G0005); `create_slot_texture` tworzy ją od razu we właściwym. Sloty na żądanie powstają tylko, gdy konsument poprosi o nie w `prepare_view`.

## `Shared`

Mapa typ → wartość. Paczka publikuje dane w `setup` albo `record`, inne je czytają; aplikacja może podać je przy budowie (`RendererBuilder::with_shared`) i zmieniać między klatkami (`Renderer::shared_mut`). Tak aplikacja przekazuje paczkom scenę bez zależności rdzenia od jakiegokolwiek ECS.

## `ViewCtx`

Rozmiar wyjścia i renderu, jitter, mip bias, kamera (`Camera`, reverse-Z) oraz czas i numer klatki od wołającego — rdzeń nie czyta zegara.

`ViewId` to tożsamość celu widoku: stała, dopóki cel żyje, i nowa po jego ponownym utworzeniu (nie zależy od pozycji widoku w klatce).

## `TaskPool`

ADR-0008: `parallel_for(len, &task)` blokuje do końca, więc zadanie może pożyczać dane klatki. `SerialTaskPool` jest domyślny (także na wasm).
