# Pojęcia

Słownik pojęć, których rdzeń i paczki używają w tym samym znaczeniu. Decyzja: [ADR-0022](docs/adr/0022-zegar-klatki-paczek.md).

## Klatka (frame)

Klatka zatwierdzona: taka, której lista komend dotarła do GPU albo do hosta. W `render_frame` klatka liczy się zaraz po udanym `submit` (także gdy późniejszy `present` zawiedzie), w `record_frame` w chwili zwrócenia listy. Klatka porzucona (błąd widoku albo `submit`) się nie liczy. Liczy je rdzeń: `Renderer::frame_index()` dla hosta, `ViewCtx::frame` dla paczek. Tym zegarem chodzi opóźnione zwolnienie.

## Epoka nagrania (record epoch)

Licznik prób nagrania: rośnie na początku każdego `render_frame` i `record_frame`, także tych, które skończą się porzuconą klatką. Wszystkie widoki jednej próby mają tę samą epokę (`ViewCtx::record_epoch`). Paczka restartuje na jej zmianie scratch per nagranie, np. pierścień uniformów, bo ponowiona próba nie może zaczynać tam, gdzie stanęła porzucona.

## Opóźnione zwolnienie (deferred release)

Kolejka rdzenia (`DeferredRelease`): obiekt GPU oddany przy klatce *N* jest niszczony dopiero po `frames_in_flight` zatwierdzonych klatkach, bo GPU może go jeszcze czytać. Paczka oddaje obiekt przez `RecordCtx::release_later`; w trybie A host odpowiada za to, żeby `with_frames_in_flight` pokrywał jego opóźnienie między `record_frame` a własnym `submit`.
