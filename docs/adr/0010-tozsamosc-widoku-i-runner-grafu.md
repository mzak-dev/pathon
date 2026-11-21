# ADR-0010: Tożsamość widoku i runner grafu

**Status:** Przyjęta
**Data:** 2026-11-21

## Kontekst

`Renderer` prowadził przebieg grafu krok po kroku: pulę tekstur, cache grafów, czyszczenie celu, fallback po błędzie i capture. Do rejestru paczek sięgał bezpośrednio (`entries`, `order`, `temporal_owner`). Błędy 1, 3, 4 i 7 z przeglądu z 20.11.2026 brały się z tych szwów.

Cache grafu był indeksowany pozycją widoku w `FrameInput::views`, a `ViewId` dla paczek był tą samą pozycją. Przestawienie albo pominięcie widoku (np. lusterko odświeżane co drugą klatkę) psuło cache, a przyszła historia TAA mieszałaby się między widokami.

## Decyzja

- **Widok = cel.** Tożsamością widoku jest jego `TargetId`. `ViewId` w `ViewCtx` to uchwyt celu: indeks i generacja w `u64`, więc cel usunięty i utworzony na nowo jest nowym widokiem. `FrameInput` nie dostaje nowych pól. Jeden widok na cel.
- **Runner grafu.** Moduł `GraphRunner` w `pathon-core` dostaje cel widoku i funkcję wypełniającą graf, a oddaje komendy, statystyki, capture i diagnostyki. W środku: pula tekstur, cache grafów per `TargetId`, czyszczenie celu, fallback i sprzątanie po błędzie. Usunięcie celu usuwa jego cache.
- **Rejestr pilnuje swoich niezmienników.** Pola `Registry` są prywatne. Renderer woła fazy `prepare_view` i `record_view`; rejestr sam wyłącza paczki, które zawiodą (F0105), i utrzymuje kolejność, role oraz właściciela roli temporalnej.

## Rozważane warianty tożsamości

- **Jawne `ViewInput::id` od hosta:** pozwala na kilka widoków w jednym celu (atlas lusterek, split-screen, ściany cubemapy), ale każdy host musi pilnować stałych id, a pomyłka miesza historię między kamerami.
- **Pozycja na liście:** bez zmian, ale niestabilna.

## Konsekwencje

- Lusterka jako osobne cele offscreen działają bez kodu po stronie hosta.
- Kilka widoków w jednym celu wymaga viewportu albo warstwy w `ViewInput`. Razem z nimi dojdzie opcjonalny klucz widoku, domyślnie równy celowi; to zmiana `ViewInput` łamiąca hosty, które budują go literałem.
- Dwa widoki z tym samym celem w jednej klatce to błąd hosta, a skutek zależy od rodzaju celu: cel offscreen — drugi widok nadpisuje pierwszy; okno — drugie pobranie klatki kończy się błędem R0111 w każdej klatce; zaimportowana tekstura — drugi widok startuje ze stanu `before`, choć pierwszy zostawił ją w `after`. Cache grafu będzie się przy tym przeliczał. Od ADR-0013 rdzeń wykrywa taki duplikat, pomija drugi widok i zgłasza F0106; ten opis skutków dotyczy stanu sprzed zmiany.
