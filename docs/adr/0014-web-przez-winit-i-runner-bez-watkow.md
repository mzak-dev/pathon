# ADR-0014: Web przez winit i runner bez wątków

**Status:** Przyjęta
**Data:** 2026-11-21

## Kontekst

M0-06 przewidywał osobny crate `pathon-platform-web` (canvas, asynchroniczny start, `requestAnimationFrame`, komunikat bez WebGPU). winit 0.30, którego już używamy, ma backend webowy: okno to canvas, pętla chodzi na `requestAnimationFrame`, a `EventLoop::spawn_app` nie blokuje wątku. Runner z fasady `pathon` (ADR-0004) trzyma renderer na osobnym wątku i tworzy urządzenie blokująco — na wasm nie ma wątków, a urządzenie powstaje tylko asynchronicznie (`RendererBuilder::build_async`). Web obsługujemy wyłącznie przez WebGPU (ADR-0003).

## Decyzja

- **Bez nowego crate'a platformy.** `pathon-platform-winit` działa także na `wasm32`: `Platform::run` używa `spawn_app` i wraca od razu, okno tworzy canvas dołączony do elementu o id `pathon` (albo do `<body>`, gdy go nie ma), a rozmiar canvasu śledzi rozmiar elementu. winit zostaje jedyną biblioteką okienkową (reguła deps-check).
- **Runner bez wątków na wasm.** `pathon::runtime::run` ma tę samą sygnaturę; na wasm buduje renderer przez `build_async` (zadanie `wasm-bindgen-futures`), do tego czasu klatki są pomijane, a potem renderuje na jedynym wątku w każdym `requestAnimationFrame`. `run_with_hook` działa tak samo.
- **Brak WebGPU:** gdy przeglądarka nie ma `navigator.gpu` albo nie daje adaptera, runner zastępuje canvas czytelnym komunikatem w stronie (po polsku i po angielsku) i kończy start bez paniki; `run` zwraca `Ok`.
- **Przykład webowy** to osobny crate `examples/web` (cdylib z `#[wasm_bindgen(start)]`), budowany `wasm-pack` ze skryptu w repo, z `wasm-opt -Oz` w profilu release. Na wejściu instaluje `console_error_panic_hook`.
- **CI:** job `wasm` buduje przykład w release i wpisuje rozmiar `.wasm` (surowy i gzip) do podsumowania joba (M0-03).

## Rozważane warianty

- **Osobny `pathon-platform-web` na `web-sys` bez winita:** drugi zestaw tłumaczenia zdarzeń i obsługi canvasu, który winit już ma.
- **Asynchroniczny `Platform::run`:** zmienia kontrakt platformy dla wszystkich celów tylko po to, by obsłużyć start na webie; asynchroniczność zostaje w runnerze.

## Konsekwencje

- Zależności tylko dla `wasm32`: `wasm-bindgen`, `wasm-bindgen-futures`, `web-sys` (wybrane cechy), `console_error_panic_hook` w przykładzie.
- Renderowanie w przeglądarce bez WebGPU jest niemożliwe z założenia; sprawdzenie obrazu wymaga Chrome lub Safari 26 z WebGPU.
- Gdy wejdzie winit 0.31 z innym modelem webowym, zmiana dotyczy tylko `pathon-platform-winit`.
