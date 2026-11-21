# pathon-devtools

Terminalowe narzędzie (ratatui) do podglądu klatki pathon na żywo: interaktywny graf passów, zasoby z aliasingiem, paczki i diagnostyki. Działa poza procesem renderera, więc nie spowalnia klatki i przeżywa jej crash.

## Uruchomienie

```sh
cargo run -p pathon-devtools -- --demo                 # renderer na backendzie null w tym samym procesie
cargo run -p pathon-devtools                           # łączy się z 127.0.0.1:7425
cargo run -p pathon-devtools -- --connect HOST:PORT
cargo run -p pathon-devtools -- --file pathon-snapshot-42.json
```

Przykład z prawdziwym oknem: `cargo run -p pathon-examples --bin vertical_slice -- --devtools`, potem w drugim terminalu `cargo run -p pathon-devtools`.

## Klawisze

| Klawisz | Działanie |
| --- | --- |
| `←↑↓→` | wybór passa w grafie (wzdłuż zależności) albo zasobu |
| `Tab`, `1`–`4` | zakładki: graf, zasoby, paczki, diagnostyka |
| `c` | pokaż lub ukryj wycięte passy |
| `v` | następny widok |
| `p` | pauza (snapshoty dalej przychodzą, ekran stoi) |
| `s` | zapis bieżącego snapshotu do `pathon-snapshot-<klatka>.json` |
| `d` | zapis grafu bieżącego widoku do `pathon-graph-<klatka>.dot` (Graphviz; wycięte passy szare i przerywane) |
| `q`, `Esc` | wyjście |

## Zakładki

- **graf** — kolumna to głębokość zależności, kolor ramki to etap, `#N` to kolejność wykonania, `◆` to pass z efektem ubocznym. Krawędzie wybranego passa świecą na żółto. Inspektor obok pokazuje dostępy (wersje zasobów `v1→v2`), zależności, odbiorców i bariery wstawione przed passem.
- **zasoby** — oś czasu po kolejności passów: `█` zapis, `▒` odczyt, `─` zasób żyje, `·` poza czasem życia. Ten sam kolor w kolumnie „pamięć” to jedna tekstura fizyczna współdzielona przez aliasing.
- **paczki** — paczki w kolejności klatki z rolą, etapem, priorytetem i powodem wyłączenia (kod diagnostyki).
- **diagnostyka** — ostrzeżenia i błędy z kolejnych klatek, powtórzenia zliczone jako `×N`.

## Podpięcie we własnym hoście

`DevtoolsServer::frame` woła się po każdym `render_frame`. Capture grafu jest włączony tylko wtedy, gdy jakiś klient jest podłączony, więc bez devtools klatka nic nie kosztuje. Snapshot idzie do klientów co 100 ms (domyślnie, `with_max_rate`), zawsze z klatki nagranej z włączonym capture.

```rust,ignore
let mut devtools = pathon_devtools::DevtoolsServer::start(pathon_devtools::DEFAULT_ADDRESS)?;
let report = renderer.render_frame(&input)?;
devtools.frame(&mut renderer, &report);
```

Z runnerem z fasady: `pathon::runtime::run_with_hook(platform, builder, windows, scene, move |renderer, report| devtools.frame(renderer, report))`.

## Protokół

JSON lines po TCP, jeden `Snapshot` na linię (`protocol.rs`). Serwer słucha tylko tam, gdzie go zbindujesz; zapis do wolnego klienta ma limit 20 ms, po którym klient jest odłączany, więc devtools nigdy nie blokują wątku renderu.
