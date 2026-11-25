# ADR-0033: Komendy indirect

**Status:** Przyjęta
**Data:** 2026-11-22

## Kontekst

Culling na GPU (M4-11) zapisuje argumenty draw w compute, a draw je czyta: liczba draw calli przestaje zależeć od CPU. `Command` miał tylko `DrawIndexedIndirect`, a spike ash (`docs/spike-ash.md`, wiersz „Indirect”) zapowiadał resztę: `DrawIndirect`, `DispatchIndirect`, multi-draw i warianty z licznikiem z GPU. Wariant z licznikiem (dwa bufory, dwa offsety, `max_count`: ~36 B) nie mieści się w limicie 32 B z [ADR-0009](0009-nagrywanie-przez-liste-komend.md). Metal nie ma natywnego multi-draw ani wariantu z licznikiem (`wgpu-hal` wywołuje `drawPrimitives…` w pętli po elementach, a `draw_indirect_count` ma na Metalu `//TODO`).

## Decyzja

- **Komendy** (`pathon-rhi-types::Command`): `DrawIndirect`, `DrawIndexedIndirect` (istniała), `DispatchIndirect`, `MultiDrawIndirect`, `MultiDrawIndexedIndirect` (`buffer`, `offset`, `count`) oraz `MultiDrawIndirectCount` i `MultiDrawIndexedIndirectCount`. Enkodery passów dostają `draw_indirect`, `multi_draw_indirect`, `multi_draw_indexed_indirect`, `multi_draw_indirect_count`, `multi_draw_indexed_indirect_count` (render) i `dispatch_indirect` (compute).
- **Limit 32 B bez zmian:** komendy z jednym buforem (`buffer: BufferId`, `offset: u64`, ewentualnie `count: u32`) mieszczą się w 32 B razem z tagiem. Warianty `…Count` niosą `Box<MultiDrawCount>` (`args`, `args_offset`, `count_buffer`, `count_offset`, `max_count`) — rzadkie (jeden na pass), więc alokacja nie boli, a `size_of::<Command>()` zostaje 32 B (pilnuje go test `command_stays_within_32_bytes`).
- **Argumenty:** `DrawIndirectArgs` (16 B), `DrawIndexedIndirectArgs` (20 B) i `DispatchIndirectArgs` (12 B) w `pathon-rhi-types` mają układ identyczny z odpowiednikami wgpu (a przez to z Vulkanem, D3D12 i Metalem): `#[repr(C)]`, same `u32` (`base_vertex` to `i32`), little-endian, ciasno upakowane. Serializacja to `to_bytes()` / `from_bytes()` na `to_le_bytes`, bez bytemucka; `SIZE` daje rozmiar elementu. Elementy multi-draw leżą jeden za drugim bez przerw.
- **Funkcje:** brak bitu `MULTI_DRAW_INDIRECT`. wgpu nie ma takiej funkcji: multi-draw bez licznika jest zawsze dostępny (natywnie DX12 i Vulkan dostają prawdziwy multi-draw, Metal i WebGPU pętlę `draw_indirect` w strumieniu komend; koszt CPU rośnie wtedy z `count`, ale nie ma żadnej zależności od danych GPU). Warianty `…Count` wymagają `Features::MULTI_DRAW_INDIRECT_COUNT` (DX12, Vulkan; nie Metal ani Web); bez niego backend zwraca `RhiError::MissingFeatures` (R0002) zamiast panikować w wgpu. `first_instance` ≠ 0 w argumentach wymaga `Features::INDIRECT_FIRST_INSTANCE` (profil Desktop go ma); bez niego wgpu pomija taki draw.
- **Wariant bez count (Metal, Web):** paczka z cullingiem rysuje `MultiDrawIndexedIndirect` po wszystkich kandydatach, a compute zeruje `instance_count` (albo `index_count`) obiektów odrzuconych — GPU takie elementy pomija. Wybór ścieżki w M4-11 idzie przez `Caps::has(MULTI_DRAW_INDIRECT_COUNT)`; ten ADR nie dodaje paczki ani cullingu.
- **Walidacja w `rhi-null`** (ostrzejsza niż wgpu, które sprawdza to dopiero przy `submit` i bez kodu diagnostycznego): komenda tylko w passie właściwego rodzaju (draw w render, dispatch w compute, z potokiem; R0104); bufor argumentów i bufor licznika mają `BufferUsage::INDIRECT` (R0105); offset każdego z nich jest wielokrotnością 4 (R0114); `offset + count × rozmiar_elementu` (dla `…Count`: `max_count`, a licznik zajmuje 4 B) mieści się w buforze, bez przepełnienia arytmetyki (R0108); warianty `…Count` wymagają `MULTI_DRAW_INDIRECT_COUNT` (R0002). Zawartości argumentów null nie sprawdza: nie wykonuje compute.
- **Graf bez zmian:** `Access::INDIRECT` istniał, a `buffer_usage_for` zamienia go na `BufferUsage::INDIRECT`; `STORAGE_WRITE → INDIRECT` daje barierę `BufferAccess` i krawędź między passami (ADR-0011). Dołożony tylko test, w którym graf sam nadaje buforowi użycia `STORAGE | INDIRECT`, a zapisane przez niego komendy przechodzą walidację null.
- **Profile** (`Profile::Desktop` / `High`) nie zyskują `MULTI_DRAW_INDIRECT_COUNT`: bit mają DX12 i Vulkan, ale nie Metal, więc to M4-11 decyduje o ścieżce przez `Caps`.

## Pomiar

Benchmark z ADR-0009 (`examples/benches/recording.rs`) powtórzony po dodaniu wariantów, plus nowa para `draw_indexed_indirect` w pętli (wprost do wgpu i przez listę komend). Wyniki i zastrzeżenia: [docs/bench](../bench/README.md). Maszyna była obciążona innymi buildami (rozrzut tej samej binarki ±20%), a narzut listy względem wgpu wprost wyszedł na niej wyższy niż w pomiarze z ADR-0009 także **przed** tą zmianą; po zmianie nie ma różnicy odróżnialnej od szumu. Próg 15% wymaga więc powtórzenia na spokojnej maszynie, zanim zapadnie decyzja o nagrywaniu wprost; do tego czasu backend wgpu zostaje przy liście komend.

## Konsekwencje

- Nowa komenda w `Command` = nowy test limitu rozmiaru i ponowny pomiar (ADR-0009 już to wymaga).
- Paczki, które chcą liczyć draw calle po stronie GPU, dostają gotowe typy argumentów; nie muszą znać layoutu wgpu.
- Kod diagnostyki: R0114 (strona w `docs/diag/`); wyjście poza bufor to istniejące R0108.
