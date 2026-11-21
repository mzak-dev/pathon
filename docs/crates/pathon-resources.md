# pathon-resources

Zarządzanie pamięcią GPU nad `DeviceApi` (M2-09). Handle'y generacyjne są w `pathon-rhi-types` (`Pool`); tutaj są narzędzia wyżej:

- **`DeferredRelease`** — opóźnione zwalnianie: zasób oddany w klatce *N* jest niszczony dopiero w klatce `N + frames_in_flight`, bo GPU może go jeszcze czytać. Po utracie urządzenia `forget_all()` czyści kolejkę bez niszczenia (handle'y i tak są martwe).
- **`RangeAllocator`** — sub-alokacja zakresów w jednym dużym buforze (first-fit z wyrównaniem, scalanie sąsiadów przy zwolnieniu). Pod pulę geometrii z M4-01.
- **`UniformRing`** — liniowy alokator małych danych uniform na klatkę; restartuje od zera w `begin_frame()`, bo zapisy kolejki wykonują się przed następnym zgłoszeniem.
- **`GpuArray<T>`** — tablica danych na GPU jako storage buffer na każdym profilu (instancje, materiały, światła). Wysyła tylko zmieniony zakres (zakres zostaje oznaczony, dopóki zapis się nie powiedzie), rośnie przez podwajanie, stary bufor oddaje do `DeferredRelease`. Paczka bez własnej kolejki `DeferredRelease` oddaje obiekty przez `RecordCtx::release_later` ([ADR-0022](../adr/0022-zegar-klatki-paczek.md)).
