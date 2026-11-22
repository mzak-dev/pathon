# Pomiary i budżety

Dwie rzeczy w jednym katalogu: **budżety i wykrywanie regresji** (M4-17, [ADR-0035](../adr/0035-budzety-i-regresje.md); dalej w tym pliku) oraz **jednorazowe pomiary** przy decyzjach projektowych (od sekcji „Pomiary jednorazowe”).

## Budżety i regresje

Trzy warstwy, od najtwardszej:

| Warstwa | Gdzie | Co pilnuje | Kiedy |
| --- | --- | --- | --- |
| Liczniki deterministyczne | CI (`cargo xtask budgets`) | komendy na klatkę, alokacje, rozmiar WASM | każdy push i PR |
| Budżety bezwzględne | `budgets.toml`: `max` lokalnie, `ci_max` w CI | „extract 10 000 instancji < 2 ms” | `xtask bench`, CI z szerokim marginesem |
| Baseline na maszynę | `baseline.json` | regresja czasu o ponad 10% względem tej samej maszyny | ręcznie: `cargo xtask bench` |

CI **nie** porównuje czasów z baseline: runner jest współdzielony i inny niż maszyny deweloperskie, więc szum przekroczyłby próg. Alarm regresji uruchamia się ręcznie przed PR-em, który dotyka wydajności.

### Uruchomienie

```sh
cargo xtask bench                       # wszystkie sondy, porównanie z baseline tej maszyny; kod 1 przy regresji
cargo xtask bench --suite scene         # jedna sonda (opcja powtarzalna)
cargo xtask bench --update              # zapisz wynik jako nowy baseline tej maszyny
cargo xtask bench --skip-gpu            # bez sond z `gpu = true`
cargo xtask budgets                     # to, co robi CI: sondy z `ci = true` względem `ci_max` / `max`
```

Zamknij inne programy, zanim zmierzysz: wynik z zajętej maszyny jest bezwartościowy. Kolumna `spread` pokazuje, jak bardzo różnią się mediany z trzech uruchomień sondy; `--update` odmawia zapisu, gdy rozrzut przekracza próg regresji (`--allow-noisy` to obchodzi). Każdy przebieg zapisuje też `target/bench/latest.json` (ten sam format co baseline).

Metodyka: każda sonda to osobny proces, uruchamiany `repeats` razy (3); w jednym uruchomieniu zgłasza co najmniej 21 próbek czasu. Wynikiem jest **mediana median**, obok niej najmniejsza próbka. Czas jest regresją tylko wtedy, gdy mediana **i** minimum urosły o więcej niż `time_pct` (szum zawyża medianę znacznie częściej niż minimum, a prawdziwe spowolnienie przesuwa oba). Liczniki i rozmiary porównujemy dokładnie.

### Maszyny

`baseline.json` trzyma wpisy po nazwie maszyny. `xtask` znajduje wpis po systemie i modelu CPU; gdy go nie ma, `--update` zakłada nowy o nazwie `<system>-<cpu>`, a `--machine NAZWA` (albo `PATHON_BENCH_MACHINE`) nadaje własną. Wpis pamięta system, CPU, listę GPU, a przy każdej sondzie: datę, commit (i czy drzewo było brudne), wersję rustc i liczbę uruchomień.

| Nazwa | Maszyna | Stan |
| --- | --- | --- |
| `windows-rx9070xt` | AMD Ryzen 5 5600G, Radeon RX 9070 XT, Windows 11 Pro | baseline zapisany (sondy CPU) na **zajętej** maszynie (`--repeats 7 --allow-noisy`, bo inne procesy zajmowały CPU); **do powtórzenia na cichej** (`cargo xtask bench --update`) |
| `macbook-air-m5` | MacBook Air M5 | **czeka na sprzęt**: `cargo xtask bench --update --machine macbook-air-m5` |

### Dodanie pomiaru

1. Sonda to `[[bench]]` z `harness = false` w crate'cie (`benches/probe.rs`), z `pathon-bench` w `[dev-dependencies]`. Jej `main` drukuje wyniki ([protokół](../crates/pathon-bench.md)): `emit(&Metric::timing(nazwa, &próbki))`, `Metric::count`, `Metric::bytes`. Licznik alokacji: `#[global_allocator]` z `CountingAllocator` i `count_allocations`.
2. Wpis w [`registry.toml`](registry.toml):

   ```toml
   [[suite]]
   id = "pbr-1440p"
   command = "cargo bench -p pathon-pbr --bench probe"
   ci = false     # true: uruchamia go też CI (tylko liczniki i czasy z szerokim marginesem)
   gpu = true     # wymaga GPU
   ```

3. Opcjonalnie budżet w `budgets.toml` (niżej). Baseline powstaje przy następnym `cargo xtask bench --update` (osobno `--suite pbr-1440p`).

### `budgets.toml`

```toml
[regression]                  # progi (ADR-0035)
time_pct = 10.0               # czas: o tyle może urosnąć mediana i minimum
count_pct = 0.0               # liczniki i rozmiary: dokładnie
min_samples = 21              # próbek czasu na jedno uruchomienie sondy
repeats = 3                   # uruchomień sondy

[regression.overrides]        # próg dla jednego pomiaru, z uzasadnieniem w ADR
"scene.extract.spawn_100k" = 15.0

[budgets."scene.extract.spawn_10k"]
max = "2 ms"                  # budżet; jednostki: ns, us, µs, ms, s, B, KiB, MiB, GiB, liczba bez jednostki
ci_max = "20 ms"              # limit CI (tylko czas); bez niego CI sprawdza `max`
```

Test, który mierzy czas, czyta budżet stąd (`pathon_bench::Budgets::load(..).max("nazwa")`), więc wartość żyje w jednym miejscu. **Podniesienie budżetu to zmiana tego pliku w PR z uzasadnieniem**; po zmianie, która celowo zmienia wynik, `cargo xtask bench --update`.

### CI

- Job `budgets` (ubuntu): `cargo xtask budgets` uruchamia sondy `ci = true` (extract sceny, pula meshy, klatka rdzenia na `rhi-null`) i sprawdza je względem `budgets.toml`. Liczniki (komendy i alokacje na klatkę, alokacje puli) są dokładne i mają kilkanaście procent zapasu; czasy przekraczają limit dopiero przy rzędzie wielkości (`ci_max`).
- Job `wasm`: ostatni krok sprawdza rozmiar release WASM (surowy i gzip) względem `wasm.raw_bytes` i `wasm.gzip_bytes`; przekroczenie czerwieni job.

### Co zostaje

Scena PBR w 1440p (benchmark GPU na RX 9070 XT i MacBooku Air M5) powstanie po M4-05; dojdzie jako jeden wpis `gpu = true` w `registry.toml` (szkic jest tam w komentarzu). Baseline na M5 wymaga sprzętu użytkownika.

## Pomiary jednorazowe

Maszyna: AMD Ryzen 5 5600G, AMD Radeon RX 9070 XT (Vulkan), Windows 11 Pro · Rust 1.99.1, profil release · divan 0.1.21, 400 próbek. Pomiar z 20.11.2026.

Dla kodu ograniczonego przez CPU **najszybszy** wynik najmniej zależy od szumu systemu (inne procesy, zegar CPU), więc to on służy do porównań; mediana jest podana obok.

```sh
cargo bench -p pathon-graph --bench compile
cargo bench -p pathon-core --bench frame
cargo bench -p pathon-examples --bench recording   # wymaga GPU
```

Inne pomiary mają własne noty: [cache potoków na dysku (M3-05)](pipeline-cache.md) — test GPU, nie divan.

## Kompilacja grafu klatki (M2-07)

Łańcuch passów postprocessu kończący się w celu renderu (`crates/pathon-graph/benches/compile.rs`).

| Passy | Na zimno (najszybszy / mediana) | Z cache (najszybszy / mediana) | Zysk |
| ---: | --- | --- | ---: |
| 16 | 1,8 µs / 2,9 µs | 0,30 µs / 0,31 µs | ~6× |
| 64 | 6,8 µs / 7,3 µs | 1,2 µs / 1,2 µs | ~6× |
| 256 | 24 µs / 39 µs | 4,5 µs / 7,5 µs | ~5× |

Cache porównuje strukturę grafu dokładnie, zamiast ją hashować. Pierwsza wersja z hashem (SipHash) była **wolniejsza** od kompilacji na zimno (69 µs wobec 42 µs przy 256 passach), bo hashowanie każdego passa kosztuje więcej niż kompilacja.

## Klatka w rdzeniu

`record_frame` z czterema paczkami testowymi (geometria, upscaler na 67%, konsument slotu na żądanie, sześcian unlit) na backendzie null, bez zgłoszenia — czyli `prepare_view`, budowa grafu przez paczki, kompilacja z cache, przydział pamięci z puli i plan barier (`crates/pathon-core/benches/frame.rs`).

| Widoki | Najszybszy | Mediana |
| ---: | --- | --- |
| 1 | 6,1 µs | 10,7 µs |
| 4 | 21,6 µs | 22,1 µs |

## Nagrywanie: lista komend pathon a wgpu wprost (M2-16)

Te same draw calle (bind group, bufor wierzchołków, bufor indeksów, `draw_indexed` na każdy) nagrane wprost do enkodera wgpu albo do `CommandList` pathon i przetłumaczone przez `interop::encode_into`. Oba warianty kończą się na `CommandEncoder::finish`, bez zgłoszenia (`examples/benches/recording.rs`).

| Draw calle | wgpu wprost (najszybszy / mediana) | przez pathon (najszybszy / mediana) | Narzut (najszybszy) |
| ---: | --- | --- | ---: |
| 100 | 52 µs / 90 µs | 56 µs / 87 µs | +6% |
| 1 000 | 418 µs / 689 µs | 437 µs / 703 µs | +4% |
| 10 000 | 4,96 ms / 7,40 ms | 5,16 ms / 6,68 ms | +4% |

Mediany różnią się w granicach szumu w obie strony. Narzut jest poniżej progu z [ADR-0009](../adr/0009-nagrywanie-przez-liste-komend.md) (15%), więc backend wgpu zostaje przy liście komend.

Przed zmniejszeniem `Command` z 80 do 32 bajtów (rzadkie, duże komendy w `Box`) narzut przy 10 000 draw callach wynosił ~31%.

## Nagrywanie po dodaniu komend indirect (M4-11, [ADR-0033](../adr/0033-komendy-indirect.md))

Ten sam benchmark (`examples/benches/recording.rs`) powtórzony po dodaniu wariantów `Command` i ich tłumaczenia w `encode.rs`; `Command` nadal ma 32 B. Dołożona para `direct_wgpu_indirect` / `via_pathon_indirect` to ten sam draw jako `draw_indexed_indirect` z bufora argumentów (jedna komenda na draw). Pomiar z 22.11.2026 na tej samej maszynie, **obciążonej** innymi procesami (kilka buildów naraz), więc liczby szumią o ±20%: ta sama binarka dawała od 5,1 do 6,2 ms przy 10 000 draw callach. Dlatego porównanie jest przeplatane (kod sprzed zmiany i po zmianie na przemian, dwie serie po 6 rund po 1000 próbek, najszybszy wynik ze wszystkich rund); kolumna „wgpu wprost” to najlepszy wynik spośród wszystkich binarek, bo ten kod jest w nich identyczny.

| Draw calle | wgpu wprost | przez pathon, przed zmianą | przez pathon, po zmianie | Narzut przed → po |
| ---: | --- | --- | --- | --- |
| 100 | 52 µs | 67 µs | 59 µs | +31% → +14% |
| 1 000 | 417 µs | 552 µs | 489 µs | +32% → +17% |
| 10 000 | 5,13 ms | 5,85 ms | 6,80 ms | +14% → +33% |

Para indirect (4 rundy po 1000 próbek, bez przeplatania): przy 100 / 1 000 / 10 000 draw callach wgpu wprost 72 µs / 463 µs / 5,60 ms, przez pathon 72 µs / 546 µs / 7,11 ms (0% / +18% / +27%).

Wnioski i zastrzeżenia:

- Zmiana nie ma mierzalnego wpływu na nagrywanie przy 100 i 1 000 draw callach (po zmianie jest nawet szybciej, co jest szumem). Przy 10 000 draw callach wyniki po zmianie są o ~16% wyższe w obu seriach (5,85 → 6,80 ms), ale rozrzut tej samej binarki (5,1–6,2 ms) jest większy, a przy 100 i 1 000 kierunek jest przeciwny, więc nie da się tego odróżnić od szumu na obciążonej maszynie.
- **Narzut względem wgpu wprost był już przed zmianą wyższy niż +4% z pomiaru z 20.11.2026** (+14…27% na obciążonej maszynie). Próg 15% z ADR-0009 jest więc na granicy i wymaga powtórzenia na spokojnej maszynie (RX 9070 XT, nic nie buduje się obok), zanim ktokolwiek zdecyduje o nagrywaniu wprost.
- Komenda `MultiDraw…` zastępuje całą pętlę draw calli jedną komendą, więc koszt CPU nagrywania przestaje zależeć od liczby draw calli; to jest zysk M4-11, a nie narzut listy.
