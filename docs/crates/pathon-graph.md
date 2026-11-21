# pathon-graph

Graf klatki (M2-01…M2-06): paczki deklarują passy i ich dostępy do tekstur, graf ustala kolejność, usuwa zbędne passy, przydziela pamięć i zapisuje plan barier.

## Zasoby i wersje (M2-01)

- `create_texture(name, GraphTextureDesc)` — zasób tymczasowy; rozmiar `Render`, `Output` albo stały. Flagi użycia (`TextureUsage`) graf wylicza sam z zadeklarowanych dostępów.
- `import_texture(name, id, desc, initial, final)` — zasób spoza grafu (okno, offscreen, tekstura hosta). Graf przyjmuje go w stanie `initial` i oddaje w `final`.
- Każdy zapis tworzy nową wersję (`TextureHandle`). `write` porzuca poprzednią treść, `modify` ją czyta (np. `LoadOp::Load`), `write_range` / `read_range` działają na zakresie mipów i warstw (Hi-Z). Zapis częściowy zależy od poprzedniej wersji, bo pozostałe mipy muszą zostać.

## Kolejność (M2-02)

Twarde krawędzie: zależności danych (RAW, WAW, WAR) oraz `before` / `after`. Zależności danych liczy się po cullingu, z łańcucha żywych dostępów do każdego zasobu, więc wycięty pośredni zapis nie rozrywa kolejności między passami przed nim i po nim. Wśród gotowych passów pierwszy jest najniższy `(Stage, priority)`; remis rozstrzyga kolejność rejestracji z ostrzeżeniem **G0006**. Ten sam algorytm (`sort_nodes`) porządkuje paczki w rdzeniu, więc łańcuch paczek na jednym slocie nie zależy od kolejności rejestracji.

## Culling (M2-03)

Korzenie to passy `side_effect()` i producenci ostatniej wersji każdego zasobu importowanego. Wszystko, od czego zależą, zostaje; reszta znika — także producent treści, którą ktoś nadpisał przez `write`. Slot, którego nikt nie czyta, kosztuje zero.

## Pamięć (M2-04)

Tekstury tymczasowe o tym samym kształcie i rozłącznych czasach życia dzielą jedną teksturę fizyczną (z sumą flag użycia). `ResourcePool` trzyma tekstury między klatkami, więc stabilna scena po pierwszej klatce nic nie alokuje; nieużywane przez 3 klatki trafiają do `DeferredRelease`.

## Bufory (ADR-0011)

Bufory są zasobami grafu jak tekstury: `create_buffer(name, GraphBufferDesc)` i `import_buffer(name, id, size, initial, final)`, a passy deklarują je przez `read_buffer` / `write_buffer` / `modify_buffer` z tą samą semantyką wersji, cullingu i kolejności. Flagi `BufferUsage` graf wylicza z `Access` (`SHADER_*` daje `STORAGE`), a bufory tymczasowe o tym samym rozmiarze i rozłącznych czasach życia dzielą pamięć w `ResourcePool` (dawny `TexturePool`). Przed passem graf wpisuje `BufferAccess` według tej samej reguły co dla tekstur, a zaimportowane bufory na końcu przechodzą do `final`; w passie id bufora daje `PassContext::buffer`.

## Bariery (M2-05)

Przed każdym passem graf porównuje potrzebny `Access` każdego subresource'u z bieżącym i dopisuje `TextureAccess`, także po każdym zapisie (zależność pamięci dla backendów natywnych). Na końcu zasoby importowane przechodzą do stanu `final` (okna: `PRESENT`). Testy wykonują ten plan na `pathon-rhi-null`, który odrzuca każdy błąd stanu.

## Walidacja (M2-06)

- **G0001** — cykl (np. `after` sprzeczne z zależnością danych);
- **G0002** — `before` / `after` wskazuje nieznany pass (ostrzeżenie);
- **G0003** — odczyt tekstury tymczasowej, której nic nie zapisało;
- **G0004** — render pass miesza załączniki w rozmiarze renderu i wyjścia;
- **G0006** — kolejność rozstrzygnięta rejestracją (ostrzeżenie).

## Nagrywanie

`PassBuilder::record` dostaje `PassContext`: `texture()`, `view()` (widoki keszowane w puli), `commands()`, `device()` i `create_bind_group()` (zwalniana po klatce). Cache skompilowanego grafu (M2-07) i historia (M2-08) są poza ścieżką minimalną — kompilacja co klatkę jest tania przy obecnych rozmiarach grafu.

## Capture

`FrameGraph::set_capture(true)` dokłada do `Execution` obraz wykonanego grafu (`GraphCapture`): passy z kolejnością albo informacją o wycięciu, dostępy z wersjami, krawędzie zależności, teksturę fizyczną każdego zasobu (aliasing), bariery przed każdym passem i na końcu klatki. Domyślnie wyłączony, bo alokuje.
