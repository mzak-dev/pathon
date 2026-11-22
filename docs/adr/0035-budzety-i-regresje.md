# ADR-0035: Budżety i wykrywanie regresji

**Status:** Przyjęta
**Data:** 2026-11-22
**Dotyczy:** [ADR-0009](0009-nagrywanie-przez-liste-komend.md) (próg 15% narzutu listy komend), [ADR-0014](0014-web-przez-winit-i-runner-bez-watkow.md) (web), [ADR-0029](0029-render-world-i-extract.md) (budżet tymczasowy extract), M4-17

## Kontekst

Wydajność ma być cechą renderera, nie przypadkiem: kolejne paczki (PBR, culling, cienie) dokładają koszt na klatkę, a hosty (Bevy, Godot, własny silnik) płacą za to w swoim budżecie klatki. Do M4-17 były tylko pomiary jednorazowe (`docs/bench/README.md`) i jedna stała z ADR-0029 (extract 10 000 instancji < 2 ms), pilnowana testem `#[ignore]`.

Dwie rzeczy utrudniają pilnowanie czasu. Po pierwsze, czas ścienny na hostowanym runnerze CI jest zaszumiony (współdzielony procesor, inny sprzęt z przebiegu na przebieg), więc próg 10% dałby fałszywe alarmy, a próg tak szeroki, żeby ich nie dawać, nie wykryłby niczego. Po drugie, wyniki różnych maszyn nie są porównywalne: docelowo RX 9070 XT z Ryzenem na Windowsie i MacBook Air M5 mają różne bezwzględne czasy i różny szum.

## Decyzja

### Trzy warstwy

1. **Liczniki deterministyczne** (CI, twarde). Liczba komend na klatkę, liczba alokacji (na wątek, licznik w `pathon-bench`) i ich rozmiar, rozmiar release WASM. Nie zależą od maszyny, więc CI pada, gdy wzrosną ponad limit z `docs/bench/budgets.toml`. To jest główna ochrona w CI.
2. **Budżety bezwzględne** (`budgets.toml`). `max` obowiązuje lokalnie (`cargo xtask bench`, testy mierzące czas); `ci_max` to osobny, szeroki limit czasu dla CI (rząd wielkości ponad pomiarem), który łapie katastrofy (np. przypadkowe O(n²)), nie 10% regresje.
3. **Baseline na maszynę** (lokalnie, ręcznie). `docs/bench/baseline.json` trzyma wyniki każdej maszyny osobno; `cargo xtask bench` porównuje z wpisem tej maszyny i kończy się kodem 1 przy regresji. **Alarm nie działa w CI** i nie porównuje maszyn między sobą.

### Próg regresji

- **Czas: 10%** względem baseline tej samej maszyny, liczony na **medianie median** z 3 uruchomień sondy (każde to osobny proces i ≥ 21 próbek, czyli ≥ 63 próbki na pomiar), a regresją jest dopiero sytuacja, gdy **mediana i minimum** urosły o więcej niż 10%. Szum (inne procesy, zegar CPU) zawyża medianę znacznie częściej niż minimum, a prawdziwe spowolnienie przesuwa oba; warunek na oba usuwa większość fałszywych alarmów bez ukrywania regresji. Poprawa o ponad 10% jest raportowana jako sygnał do `--update`, nie jako błąd.
- **Liczniki i rozmiary: 0%.** Są deterministyczne, więc każdy wzrost to zmiana kształtu pracy: `--update` po przeglądzie, nie po cichu.
- Wartości są w `budgets.toml` (`[regression]`); wyjątek dla jednego pomiaru (`[regression.overrides]`) wymaga uzasadnienia w tym ADR, bo próg, który da się podnieść bez śladu, przestaje chronić.
- 10% jest mniejsze niż próg 15% z ADR-0009: tamten dotyczył jednorazowej decyzji o narzucie abstrakcji względem wgpu wprost, ten wykrywa dryf kolejnych zmian, które razem dają 15%.
- `xtask bench --update` odmawia zapisu baseline, gdy mediany trzech uruchomień jednej sondy różnią się o więcej niż próg (maszyna była zajęta); baseline zapisany na zajętej maszynie zawyżałby wynik i maskował późniejsze regresje. `--allow-noisy` to obchodzi, świadomie.

### Baseline: format i maszyny

- Wpis na maszynę, klucz to nazwa (`windows-rx9070xt`, `macbook-air-m5`); wpis zawiera system, CPU, listę GPU i dla każdej sondy: datę, commit (z informacją o niezatwierdzonych zmianach), wersję `rustc`, liczbę uruchomień i metryki (jednostka, mediana, minimum, liczba próbek). Sondę zapisuje się w całości, więc częściowe `--update --suite` nie zostawia metryki obok commita, z którym nie była mierzona.
- Wpis dla maszyny `xtask` znajduje po systemie i CPU; nowa maszyna dostaje nazwę z systemu i CPU albo własną (`--machine`). **Baseline dla MacBooka Air M5 nie istnieje**: powstanie jednym `cargo xtask bench --update --machine macbook-air-m5` na tym sprzęcie.
- Sondy to dowolne komendy drukujące linie protokołu (`pathon-bench`); rejestr (`docs/bench/registry.toml`) to dane. Nowa paczka dopisuje pomiar jednym wpisem `[[suite]]` (`id`, `command`, `ci`, `gpu`), bez zmian w `xtask`.

### Rozmiar WASM

`wasm.raw_bytes` ≤ 800 KiB i `wasm.gzip_bytes` ≤ 330 KiB dla `wasm-pack build examples/web --release` (lokalnie 692 KiB i 283 KiB; zapas ok. 16% na różnice toolchaina CI). Job `wasm` pada przy przekroczeniu. **Podniesienie limitu to zmiana `budgets.toml` w PR z uzasadnieniem** (co dołożyło rozmiar i dlaczego warto), nie zmiana workflow.

### Budżet extract (z ADR-0029)

`scene.extract.spawn_10k` i `scene.extract.move_10k`: `max = 2 ms` w `budgets.toml` zastępuje stałą z testu; test `crates/pathon-scene/tests/budget.rs` czyta ją stamtąd. Wartość 2 ms zostaje, bo pomiar (spawn ≈ 1,2 ms, ruch ≈ 0,2 ms) jej nie podważył; to jest teraz budżet, nie tymczasowa wartość.

### Czego ta decyzja jeszcze nie obejmuje

Benchmark GPU: scena PBR w 1440p na RX 9070 XT i MacBooku Air M5. Scena powstaje w M4-05 i M4-10; sonda dojdzie jako wpis `gpu = true` w rejestrze (z `--skip-gpu` dla maszyn bez GPU), z własnym baseline na każdej maszynie. Czasy GPU na hostowanym runnerze nie wchodzą do CI.

## Rozważane warianty

- **Czas ścienny jako bramka CI z progiem 10%:** flaky; albo ciągle czerwony, albo próg tak szeroki, że bezużyteczny. Zostały liczniki i szerokie `ci_max`.
- **Baseline wspólny dla maszyn (przeliczany względem kalibracji):** każda kalibracja jest kolejnym zaszumionym pomiarem; osobny baseline na maszynę jest prostszy i uczciwszy.
- **`criterion` / `divan` + parsowanie wyjścia:** divan nie ma wyjścia maszynowego, a ich statystyka (bootstrap, outliery) nie jest potrzebna: mediana median i minimum wystarczą i są przewidywalne. Protokół `pathon-bench` to jedna linia na pomiar.
- **Alarm w PR przez CI (bot):** wymaga stałej maszyny z GPU, której nie ma; ręczne uruchomienie przed PR-em wystarczy na tym etapie.
- **Liczenie alokacji wspólnym licznikiem globalnym:** wątki harnessu zaburzałyby wynik; licznik na wątek daje liczbę dokładną.

## Konsekwencje

- Każda paczka z kosztem na klatkę (PBR, culling, cienie) dopisuje sondę i wpis w `registry.toml`; wzrost licznika w CI wymaga świadomej zmiany `budgets.toml`.
- Baseline wchodzi do repo razem ze zmianami, które celowo zmieniają wynik; historia `git log docs/bench/baseline.json` to historia wydajności.
- Maszyna użyta do `--update` musi być cicha; to dyscyplina, nie coś, co CI wymusi.
- Nowy crate narzędziowy `tools/pathon-bench` (dev-zależność crate'ów z sondami) i zależność `toml` w `xtask`; domknięcie zależności `pathon-core` bez zmian (`deps-check` zielony).
