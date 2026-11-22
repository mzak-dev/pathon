# pathon-golden

Infrastruktura golden images (M4-00): obraz z renderu headless porównywany z referencją PNG w repozytorium. Crate jest **tylko deweloperski** (`publish = false`, w `[dev-dependencies]` testów), więc nie wchodzi do domknięcia `pathon-core` ani do żadnej paczki; decyzje o metryce i polityce referencji: [ADR-0025](../adr/0025-golden-images.md).

## Użycie

```rust,ignore
use pathon_golden::{GoldenImage, Tolerance, assert_golden, read_texture};
use pathon_rhi::Access;

let image = read_texture(renderer.device().as_ref(), texture, Access::SAMPLED)?;
assert_golden("unlit_cube", &image, Tolerance::cross_backend());
```

- `assert_golden(name, &image, tolerance)` porównuje z `tests/golden/<name>.png`; przy niezgodności panikuje z raportem (liczba i odsetek różniących się pikseli, największa różnica kanału, pierwszy różny piksel, ścieżki plików).
- Obraz nie musi pochodzić z `pathon-core`: `GoldenImage::from_rgba8(width, height, bytes)` przyjmuje bajty RGBA8 z dowolnego urządzenia i hosta (tryb B z M4-20 czyta własną teksturę wgpu i wchodzi tą drogą). `read_texture` to wygodny odczyt z `DeviceApi` dla tekstur RGBA8 i BGRA8.
- Nazwa to litery ASCII, cyfry, `_` i `-`; jedna nazwa = jeden plik.

## Metryka i próg

Piksel „różni się”, gdy dowolny kanał (RGBA) jest dalej niż `max_channel_delta` od referencji; test przechodzi, gdy różni się nie więcej niż `max_differing_fraction` pikseli. Progi są per test:

| Konstruktor | Delta kanału | Odsetek pikseli | Kiedy |
| --- | --- | --- | --- |
| `Tolerance::exact()` | 0 | 0 | to samo urządzenie i sterownik, obliczenia bez rasteryzacji |
| `Tolerance::cross_backend()` | 6 | 3% | referencja wspólna dla lavapipe, WARP, Metalu i GPU (zaokrąglenia, pokrycie krawędzi) |
| `Tolerance::new(delta, odsetek)` | dowolne | dowolny | własny próg testu |

## Aktualizacja referencji

```sh
PATHON_UPDATE_GOLDEN=1 cargo test -p pathon-core --features gpu-tests --test vertical_slice_gpu -- --test-threads=1
```

Tworzy brakujące referencje i nadpisuje te, które nie są identyczne z obrazem. Zawsze obejrzyj zmieniony PNG w diffie przed commitem.

## Niezgodność

Obrazy trafiają do `target/golden-artifacts/` (CI wysyła je jako artefakt zadania): `<nazwa>.actual.png`, `<nazwa>.expected.png` i `<nazwa>.diff.png` (ciemna kopia referencji tam, gdzie piksele są równe, niebieski w granicy progu, czerwony poza nim). Zmienione rozmiary dają błąd z rozmiarami i zapisanym `actual`; brak referencji daje błąd z podpowiedzią `PATHON_UPDATE_GOLDEN=1`.

## Referencje per backend

Domyślnie jedna referencja na wszystkie backendy. Gdy jakiś backend nie mieści się w progu, jego referencja leży w `tests/golden/<wariant>/<nazwa>.png` i ma pierwszeństwo, a wybiera ją `PATHON_GOLDEN_VARIANT=<wariant>` (aktualizacja z tą zmienną zapisuje w katalogu wariantu).

## Zmienne środowiskowe

| Zmienna | Znaczenie |
| --- | --- |
| `PATHON_UPDATE_GOLDEN` | `1` — twórz i nadpisuj referencje |
| `PATHON_GOLDEN_VARIANT` | katalog wariantu (referencje per backend) |
| `PATHON_GOLDEN_DIR` | inny katalog referencji (domyślnie `tests/golden/`) |
| `PATHON_GOLDEN_ARTIFACTS` | inny katalog artefaktów (domyślnie `target/golden-artifacts/`) |
| `PATHON_REQUIRE_GPU` | `1` — brak adaptera GPU w `gpu_or_skip` kończy test porażką zamiast pominięciem |

## Brak GPU

`gpu_or_skip(test, wynik_budowy, czy_brak_adaptera)` zamienia brak adaptera w jawne pominięcie (komunikat `SKIPPED …`, w GitHub Actions także adnotacja `::warning::`). Zadania CI, które mają mieć GPU (lavapipe, WARP), ustawiają `PATHON_REQUIRE_GPU=1`, więc brak adaptera jest tam porażką. Libtest nie ma wyniku „pominięty”: pominięty test jest zielony, dlatego komunikat widać tylko z `--nocapture`.
