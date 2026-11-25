# pathon-graph

Graf klatki (M2-01…M2-08): paczki deklarują passy i ich dostępy do tekstur i buforów oraz historię widoku, graf ustala kolejność, usuwa zbędne passy, przydziela pamięć i zapisuje plan barier.

## Zasoby i wersje (M2-01)

- `create_texture(name, GraphTextureDesc)` — zasób tymczasowy; rozmiar `Render`, `Output` albo stały. Flagi użycia (`TextureUsage`) graf wylicza sam z zadeklarowanych dostępów: `SAMPLED` daje `SAMPLED`, `STORAGE_READ` i `STORAGE_WRITE` dają `STORAGE` (dla buforów także `BufferUsage::STORAGE`).
- `import_texture(name, id, desc, initial, final)` — zasób spoza grafu (okno, offscreen, tekstura hosta). Graf przyjmuje go w stanie `initial` i oddaje w `final`.
- Każdy zapis tworzy nową wersję (`TextureHandle`). `write` porzuca poprzednią treść, `modify` ją czyta (np. `LoadOp::Load`), `write_range` / `read_range` działają na zakresie mipów i warstw (Hi-Z). Zapis częściowy zależy od poprzedniej wersji, bo pozostałe mipy muszą zostać.

## Kolejność (M2-02)

Twarde krawędzie: zależności danych (RAW, WAW, WAR) oraz `before` / `after`. Zależności danych liczy się po cullingu, z łańcucha żywych dostępów do każdego zasobu, więc wycięty pośredni zapis nie rozrywa kolejności między passami przed nim i po nim. Wśród gotowych passów pierwszy jest najniższy `(Stage, priority)`; remis rozstrzyga kolejność rejestracji z ostrzeżeniem **G0006**. Ten sam algorytm (`sort_nodes`) porządkuje paczki w rdzeniu, więc łańcuch paczek na jednym slocie nie zależy od kolejności rejestracji.

## Culling (M2-03)

Korzenie to passy `side_effect()` i producenci ostatniej wersji każdego zasobu importowanego. Wszystko, od czego zależą, zostaje; reszta znika — także producent treści, którą ktoś nadpisał przez `write`. Slot, którego nikt nie czyta, kosztuje zero.

## Pamięć (M2-04)

Tekstury tymczasowe o tym samym kształcie i rozłącznych czasach życia dzielą jedną teksturę fizyczną (z sumą flag użycia). `ResourcePool` trzyma tekstury między klatkami, więc stabilna scena po pierwszej klatce nic nie alokuje; nieużywane przez 3 klatki trafiają do `DeferredRelease`.

## Bufory (ADR-0011)

Bufory są zasobami grafu jak tekstury: `create_buffer(name, GraphBufferDesc)` i `import_buffer(name, id, size, initial, final)`, a passy deklarują je przez `read_buffer` / `write_buffer` / `modify_buffer` z tą samą semantyką wersji, cullingu i kolejności. Flagi `BufferUsage` graf wylicza z `Access` (`SHADER_*` daje `STORAGE`), a bufory tymczasowe o tym samym rozmiarze i rozłącznych czasach życia dzielą pamięć w `ResourcePool` (dawny `TexturePool`). Przed passem graf wpisuje `BufferAccess` według tej samej reguły co dla tekstur, a zaimportowane bufory na końcu przechodzą do `final`; w passie id bufora daje `PassContext::buffer`. Bufor argumentów indirect to zwykły bufor grafu: pass compute deklaruje `write_buffer(args, Access::STORAGE_WRITE)`, pass draw lub dispatch `read_buffer(args, Access::INDIRECT)`; graf nadaje buforowi `BufferUsage::STORAGE | INDIRECT`, układa passy w tej kolejności i wpisuje barierę `STORAGE_WRITE → INDIRECT` ([ADR-0033](../adr/0033-komendy-indirect.md)).

## Historia (ADR-0011, M2-08)

`FrameGraph::for_view(&HistoryStore, sizes)` tworzy graf, który zna historię widoku; `history_texture(name, GraphTextureDesc)` zwraca `HistoryTexture { previous, current, previous_valid }`. `previous` to treść z poprzedniej klatki (tylko do odczytu), `current` to tekstura do zapisu w tej klatce; pass zapisujący ostatnią wersję `current` jest korzeniem cullingu. W grafie z `FrameGraph::new()` `previous_valid` jest zawsze `false`.

**Kontrakt:** `previous_valid == true` znaczy, że `previous` zawiera dokładnie to, co ten widok zapisał do `current` w swojej ostatniej klatce, która faktycznie została wysłana. Samo istnienie pary tego nie dowodzi: ważność ustawia dopiero klatka, w której żywy pass zapisał `current` i `execute` się powiodło. Paczka czyta `previous` tylko przy `previous_valid`; gdy mimo to go czyta, dostaje zera (kolor: przezroczysta czerń, głębia: 0.0), nigdy zawartość przypadkowej pamięci.

Nazwa wskazuje jedną parę w obrębie widoku i klatki: powtórne `history_texture` z tym samym opisem zwraca tę samą parę, a z innym opisem kończy kompilację błędem **G0007**.

Fizyczne tekstury żyją w `HistoryStore` (runner trzyma po jednym na cel), poza `ResourcePool`, razem ze stanem `Access` i flagą ważności każdej pary. Para powstaje od razu z flagami `RENDER_TARGET | SAMPLED | COPY_SRC | COPY_DST`, żeby druga klatka nie dorzucała użycia i nie odtwarzała jej; `execute` tworzy ją od nowa tylko przy zmianie opisu, rozmiaru albo gdy pass chce użycia spoza tego zestawu (np. `STORAGE`), a stara trafia do `DeferredRelease`. Nowa albo nieważna `previous` jest zerowana przed pierwszym odczytem. Po udanej klatce graf zapisuje stany, a jeśli żywy pass zapisał `current`, zamienia role pary i oznacza ją jako ważną; nazwy, których klatka nie zadeklarowała, są zwalniane, bo wróciłyby z nieaktualną treścią. Klatka, która się nie powiodła, usuwa swoje pary, bo jej komendy nie zostaną wykonane.

Widok pominięty w klatce (brak `execute` dla celu) nie rusza historii; po utracie urządzenia `forget_all` czyni ją nieważną. Historia zapisana w `execute` jest prawdziwa dopiero, gdy klatka dotarła do GPU: renderer woła `GraphRunner::commit_frame` zaraz po udanym `submit`, a `abandon_frame` wszędzie tam, gdzie porzuca nagraną klatkę (błąd późniejszego widoku albo `submit`; błąd `present` po udanym `submit` nie porzuca, bo lista się wykonała), co zwalnia historię widoków, które w niej zapisały. Przy użyciu grafu bez renderera ten obowiązek spada na wołającego: po porzuconej klatce trzeba wywołać `HistoryStore::release_all`.

Tak samo działa pamięć stanów tekstur w `ResourcePool` (stan, w którym klatka zostawiła teksturę, jest `from` pierwszej bariery następnej klatki). `execute` zapisuje ją jako oczekującą; kolejny widok tej samej klatki widzi stany oczekujące, więc listy widoków łączą się poprawnie. `GraphRunner::commit_frame` zatwierdza je (`ResourcePool::commit_states`), a `abandon_frame` odrzuca (`ResourcePool::abandon_states`), więc po porzuconej klatce następna zaczyna od stanów, w jakich GPU naprawdę jest. Używając grafu bez renderera, wołaj te dwie metody po zgłoszeniu albo porzuceniu list. Odrzucona przez `rhi-null` lista nie zmienia żadnego stanu tekstury (jak GPU, który nie wykonuje błędnego bufora komend). Jedyny przypadek, w którym `previous_valid` może przesadzić, to pierwszy pass w widoku żądający użycia spoza zestawu (np. `STORAGE`) przy już ważnej historii: para zostanie odtworzona i wyzerowana, choć flaga była prawdziwa w chwili deklaracji.

## Bariery (M2-05)

Przed każdym passem graf porównuje potrzebny `Access` każdego subresource'u z bieżącym i dopisuje `TextureAccess { from, to }`, gdzie `from` to bieżący stan (jedna komenda na cały zakres tylko wtedy, gdy wszystkie subresource'y mają ten sam stan poprzedni i docelowy; stan, w którym klatka zostawiła teksturę z puli, pamięta pula), także po każdym zapisie (zależność pamięci dla backendów natywnych). Na końcu zasoby importowane przechodzą do stanu `final` (okna: `PRESENT`). Testy wykonują ten plan na `pathon-rhi-null`, który odrzuca każdy błąd stanu.

## Walidacja (M2-06)

- **G0001** — cykl (np. `after` sprzeczne z zależnością danych);
- **G0002** — `before` / `after` wskazuje nieznany pass (ostrzeżenie);
- **G0003** — odczyt zasobu tymczasowego (tekstury lub bufora), którego nic nie zapisało;
- **G0004** — render pass miesza załączniki w rozmiarze renderu i wyjścia;
- **G0006** — kolejność rozstrzygnięta rejestracją (ostrzeżenie);
- **G0007** — ta sama nazwa historii zadeklarowana z różnymi opisami;
- **G0008** — pass (albo import: `initial` / `final`) deklaruje stan tekstury, który nie przechodzi `is_valid_texture_state`, np. `SAMPLED` z `COPY_SRC`.

## Nagrywanie

`PassBuilder::record` dostaje `PassContext`: `texture()`, `view()` (widoki keszowane w puli), `commands()`, `device()` i `create_bind_group()` (zwalniana po klatce).

## Capture

`FrameGraph::set_capture(true)` dokłada do `Execution` obraz wykonanego grafu (`GraphCapture`): passy z kolejnością albo informacją o wycięciu, dostępy z wersjami, krawędzie zależności, zasoby (tekstury, bufory i historia, rozróżnione przez `CapturedKind`; `imported` dotyczy tylko tego, co przekazał host, więc historia się do niego nie liczy), fizyczną teksturę lub bufor każdego zasobu (aliasing), bariery przed każdym passem i na końcu klatki. Domyślnie wyłączony, bo alokuje.

## Eksport do DOT

`to_dot(&GraphCapture)` zwraca graf w formacie Graphviz: węzeł na pass z etykietą `nazwa`, etap i `#kolejność`, krawędzie z `capture.edges`, passy wycięte szare i przerywane. Niższy poziom, `render_dot(&[DotPass], &[(usize, usize)])`, bierze same nazwy, etapy i krawędzie, więc korzystają z niego też devtools, które mają snapshoty, a nie `GraphCapture` (klawisz `d`). Eksport do JSON to snapshoty devtools (klawisz `s`).

## Obserwator (ADR-0026)

`FrameGraph::set_observer(Some(observer))` (renderer robi to sam z obserwatora rendera) sprawia, że `execute` woła `FrameObserver::begin_pass(nazwa)` i `end_pass(nazwa)` wokół domknięcia `record` każdego żywego passu — zawsze sparowane, także gdy `record` zwróci błąd — a po udanym nagraniu `gpu_pass(urządzenie, nazwa_passu, etykieta_gpu_passu)` dla każdego `BeginRenderPass` / `BeginComputePass` bez własnych `timestamps`; zwrócone `PassTimestamps` trafiają do opisu passu (przez `CommandList::commands_mut`). Passy, które mają własne timestampy, zostają nietknięte. Trait ma puste implementacje domyślne i żyje tu, bo tu wykonują się passy; implementuje go `pathon-profile`.
