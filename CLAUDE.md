# pathon

Ultra-modularny i wydajny silnik renderowania w Rust (wgpu 30), rozszerzany paczkami. Rdzeń składa klatkę z tego, co zadeklarują paczki, i nie zna żadnej techniki renderowania; nie ma też własnej pętli ani wątku, więc działa samodzielnie, we własnym silniku, w Bevy i w Godocie.

- **Plan:** `docs/plan-implementacji.md` — kolejność tasków, zasady obowiązujące w każdym tasku i granice zależności. „Gotowe gdy” to definicja ukończenia tasku; po ukończeniu odhacz go w planie.
- **Decyzje:** `docs/adr/` — przeczytaj ADR, zanim zmienisz obszar, którego dotyczy: licencja, wątki, hosty, shadery, logowanie, pula wątków. Nowa decyzja architektoniczna = nowy ADR z kolejnym numerem.
- **Rust:** skill `mzak-dev-rust-style`.
- Dokumentacja po polsku, kod i identyfikatory po angielsku.

## Komentarze i dokumentacja

- **Kod jest samokomentujący.** Nazwy funkcji, typów, pól i zmiennych mówią, co robią i co zwracają. Gdy funkcja potrzebuje opisu, podziel ją na mniejsze o mówiących nazwach.
- **Komentarz tylko przy skomplikowanym miejscu:** wyjaśnia *dlaczego* (obejście błędu sterownika, kolejność barier, matematyka, niezmiennik `unsafe`). Jedna, dwie linie. Dotyczy też `///` na publicznym API.
- **Reszta w `docs/` jako Markdown:** opis modułów i crate'ów, architektura, kontrakty (np. sloty), przykłady użycia, strony paczek i kodów diagnostyk.
- Strona crate'u na docs.rs bierze treść z `docs/` przez `#![doc = include_str!("…/docs/….md")]`, żeby opis był w jednym miejscu.
