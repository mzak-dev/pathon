# ADR-0011: Bufory i tekstury trwałe w grafie

**Status:** Przyjęta
**Data:** 2026-11-21

## Kontekst

Graf zna tylko tekstury tymczasowe i zaimportowane. Paczki z M4 i M5 potrzebują dwóch rzeczy, których nie da się dziś zapisać bez śledzenia stanów barier w paczce:

- **bufory** — culling na GPU i bufor indirect (M4-11), przypisanie świateł (M4-05), skinning (M4-12). Pass compute, który tylko zapisuje bufor czytany przez draw indirect, nie ma dziś krawędzi i zostałby wycięty;
- **treść z poprzedniej klatki** — historia TAA (M5-01), ekspozycja, reprojekcja. Paczka musiałaby sama trzymać teksturę i pamiętać jej `Access` między klatkami.

Przegląd architektury z 20.11.2026 (kandydat 2) wskazał, że zmiana `PassBuilder` po napisaniu paczek byłaby zmianą łamiącą.

## Decyzja

- **Bufory są zasobami grafu** na tych samych zasadach co tekstury: `create_buffer(name, GraphBufferDesc)` (tymczasowy, z puli) i `import_buffer(name, id, size, initial, final)`. `PassBuilder` dostaje `read_buffer`, `write_buffer`, `modify_buffer`; każdy zapis tworzy nową wersję, więc kolejność, culling i aliasing pamięci działają bez zmian. Graf wstawia `Command::BufferAccess` przed passem, którego dostęp zmienia stan bufora. `PassContext::buffer(handle) -> BufferId`.
- **Historia** to tekstura trwała z dwoma wersjami: `history_texture(name, GraphTextureDesc) -> HistoryTexture { previous, current, previous_valid }`. `previous` to treść z poprzedniej klatki tego samego widoku (tylko do odczytu), `current` to tekstura do zapisu w tej klatce; po klatce się zamieniają. Pass zapisujący `current` jest korzeniem cullingu (treść czyta następna klatka).
- **Tekstury historii żyją w runnerze per cel** (ADR-0010), poza grafem jednej klatki. Graf pamięta ich stan `Access` między klatkami, więc paczka nie widzi barier.
- **Unieważnienie:** `previous_valid` jest `false` w pierwszej klatce, po zmianie rozmiaru albo opisu tekstury, po `reset` urządzenia i po usunięciu celu. Paczka wtedy nie czyta `previous` (np. TAA bierze bieżącą klatkę bez akumulacji).

## Rozważane warianty

- **Historia na N klatek (`history(slot, n)` z planu M2-08):** na razie wystarcza jedna poprzednia klatka; N dojdzie, gdy paczka będzie tego potrzebować.
- **Pula na maksymalny rozmiar z viewportem przy dynamicznej rozdzielczości:** odłożone do paczki, która zmienia `render_size` co klatkę. Teraz zmiana rozmiaru odtwarza teksturę i unieważnia historię.
- **Cięcie kamery:** host nie zgłasza go jeszcze w `ViewInput`; paczka sama decyduje, czy ufa `previous`.
- **Bufory trwałe (np. dane z poprzedniej klatki dla GPU-driven):** poza zakresem; paczka importuje własny bufor.

## Konsekwencje

- `PassBuilder` i `PassContext` rosną o metody buforów; API tekstur się nie zmienia.
- Walidacja stanów buforów w backendzie null pozostaje na poziomie poprawności samego stanu (`is_valid_state`); pełne śledzenie użycia buforów przez bind groupy to osobne zadanie.
- `GraphCapture` i devtools pokazują bufory i historię obok tekstur.
