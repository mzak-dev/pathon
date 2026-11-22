# pathon-rhi-null

Backend RHI bez GPU (M1-04). Wydaje handle'y i nic nie rysuje, ale przy `submit` przechodzi listę komend jak GPU i odrzuca błędy, których wgpu nie zgłosi, bo sam śledzi stany:

- **maszyna stanów per subresource** (mip × warstwa): użycie w stanie, który na to nie pozwala → `R0101`; odczyt treści, której nic nie zapisało (np. mip Hi-Z przed zapisem, `LoadOp::Load` na świeżej teksturze) → `R0102`; niepoprawna kombinacja `Access` albo bariera, której `from` różni się od śledzonego stanu subresource'a → `R0103` (po `write_texture` subresource jest w `COPY_DST`);
- **struktura listy**: komenda w złym rodzaju passa, draw bez pipeline'u, niezamknięty pass albo grupa debug → `R0104`;
- **flagi użycia** zasobów (`TextureUsage`, `BufferUsage`) → `R0105`;
- **prezentacja** klatki surface'a, która nie przeszła do `PRESENT` → `R0106`;
- **timestampy passów** (`PassTimestamps`): nieistniejący zbiór zapytań → `R0003`, `begin_index == end_index` albo indeks poza zbiorem → `R0104`; `create_query_set` bez `TIMESTAMP_QUERY` → `R0002` (jak wgpu);
- **komendy indirect**: bufor argumentów (i licznika) bez `BufferUsage::INDIRECT` → `R0105`; offset niewyrównany do 4 B → `R0114`; `offset + count × rozmiar_elementu` poza buforem (w wariantach `…Count`: `max_count` i 4 B licznika) → `R0108`; warianty `…Count` bez `MULTI_DRAW_INDIRECT_COUNT` → `R0002`; zawartości argumentów null nie sprawdza (nie wykonuje compute);
- **układ texeli** kopii: `bytes_per_row` niemieszczący wiersza albo niepodzielny przez 256 w kopii bufor↔tekstura, `rows_per_image` mniejsze niż wysokość kopii (zawsze) → `R0113`.

`submit` jest atomowy: odrzucona lista nie zmienia ani stanów tekstur, ani bajtów emulowanych buforów (jak GPU, który nie wykonuje błędnego bufora komend).

Bufory nie mają maszyny stanów (graf zarządza na razie tylko teksturami), ale `write_buffer` i kopie bufor → bufor są emulowane, więc `read_buffer` zwraca prawdziwe bajty. `simulate_device_loss()` symuluje utratę urządzenia (M1-03).

Testy grafu i rdzenia działają na tym backendzie, więc każdy plan barier jest walidowany w każdym teście (M2-05).
