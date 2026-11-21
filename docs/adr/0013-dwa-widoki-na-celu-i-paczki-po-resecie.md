# ADR-0013: Dwa widoki na jednym celu i paczki po resecie urządzenia

**Status:** Przyjęta
**Data:** 2026-11-21

## Kontekst

ADR-0010 ustalił „jeden widok na cel”, ale nie mówił, co robi rdzeń, gdy host poda dwa widoki z tym samym celem w jednej klatce. Skutek zależał od rodzaju celu: okno kończyło każdą klatkę błędem R0111, a zaimportowana tekstura dostawała zły stan wejściowy.

Paczka, której `record` zwrócił błąd, jest wyłączana z F0105 na stałe. Gdy błąd wynikał z utraty urządzenia, paczka zostawała wyłączona także po `recover()` / `reset()`, choć na nowym urządzeniu działałaby poprawnie.

## Decyzja

- **Drugi widok z tym samym celem w klatce jest pomijany** z ostrzeżeniem **F0106** w `FrameReport::diagnostics`; liczy się do `skipped_views`. Pierwszy widok renderuje się normalnie, a klatka nie kończy się błędem. Dotyczy `render_frame` i `record_frame`.
- **`reset` przywraca paczki wyłączone z F0105.** Takie paczki przechodzą ponownie `setup` na nowym urządzeniu i wracają do kolejności klatki według tych samych reguł (etap, ograniczenia, priorytet, role). Paczki wyłączone innymi kodami (F0101, F0102, F0103, F0104, F0201) zostają wyłączone. Jeśli błąd `record` się powtórzy, paczka znów dostanie F0105.

## Rozważane warianty

- **Błąd całej klatki przy duplikacie celu:** jeden błąd hosta zatrzymywałby wszystkie widoki, także poprawne.
- **Przywracanie tylko paczek wyłączonych w klatce zakończonej utratą urządzenia:** dokładniejsze, ale rdzeń musiałby pamiętać, w której klatce i z jakiego powodu paczka zawiodła; zwykły błąd paczki i tak wróci po resecie jako F0105.

## Konsekwencje

- Host, który przez pomyłkę poda cel dwa razy, widzi ostrzeżenie zamiast błędów co klatkę.
- Paczka z błędem logiki, który nie zależy od urządzenia, po resecie zawiedzie jeszcze raz i zostanie wyłączona ponownie — jedno dodatkowe F0105 na reset.
