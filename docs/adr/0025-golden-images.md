# ADR-0025: Golden images — metryka i polityka referencji

**Status:** Przyjęta
**Data:** 2026-11-22
**Dotyczy:** [ADR-0005](0005-hosty-i-tryby-integracji.md) (tryb B: obraz z urządzenia hosta), M4-00, M4-05…M4-20

## Kontekst

Od M4 każdy etap kończy się „golden image”: obraz z renderu headless porównany z referencją w repozytorium. CI renderuje na kilku stosach (lavapipe na Linuksie, WARP na Windows, Metal na macOS, GPU deweloperów), a różne rasteryzatory dają różne bity na krawędziach trójkątów i przy zaokrągleniach. Trzeba zdecydować: jedna referencja z tolerancją czy referencja per backend, i jaką metryką mierzyć różnicę. Rdzeń ma nie rosnąć o zależności (`crate-budget`, ADR-0001).

## Decyzja

- **Metryka tolerancyjna, nie percepcyjna.** Piksel różni się, gdy dowolny kanał RGBA jest dalej niż `max_channel_delta` od referencji; obraz przechodzi, gdy różni się nie więcej niż `max_differing_fraction` pikseli. Dwa progi są per test (`Tolerance`), z gotowymi `exact()` i `cross_backend()`. FLIP odrzucony na razie: jest metryką dla obrazów oglądanych (HDR, odległość od ekranu), ciągnie własne zależności i ukrywa w jednej liczbie to, co w teście jest ważne (ile pikseli i o ile). Obrazy testowe są małe (64×64 i podobne) i w LDR; jeśli któryś test HDR (bloom, ekspozycja) będzie wymagał metryki percepcyjnej, dojdzie jako druga metryka w tym samym crate'cie, bez zmiany `assert_golden`.
- **Jedna referencja na wszystkie backendy, referencje per backend tylko z konieczności.** Referencja leży w `tests/golden/<nazwa>.png`. Gdy backend nie mieści się w progu `cross_backend()` (i prób poszerzenia progu nie da się uzasadnić), jego referencja leży w `tests/golden/<wariant>/<nazwa>.png` i ma pierwszeństwo, a wariant wybiera `PATHON_GOLDEN_VARIANT`. Wariant jest wyjątkiem z opisem w tej ADR (poniżej), nie domyślną polityką: referencje per backend mnożą pliki i zacierają regresje, które objawiają się tylko na jednym backendzie.
- **Crate `pathon-golden` w `crates/`, tylko deweloperski** (`publish = false`, wpinany przez `[dev-dependencies]`): nie wchodzi do domknięcia `pathon-core`, więc `crate-budget` zostaje bez zmian. Nie jest paczką, więc nie podlega `pack-boundary`. Jedyna nowa zależność to `png` (MIT lub Apache-2.0, czysty Rust). Nie rozszerzamy `pathon-test`: to paczka w domknięciu kontraktu paczek, a golden images nie mają jej dotyczyć.
- **API dla M4-05…M4-20:** `assert_golden(name, &image, tolerance)`. `GoldenImage::from_rgba8(width, height, bytes)` przyjmuje obraz z dowolnego urządzenia i hosta (tryb B w M4-20 czyta własną teksturę wgpu), a `read_texture(&dyn DeviceApi, texture, state)` czyta teksturę RGBA8 / BGRA8 z urządzenia pathon. Test nie zależy od tego, kto stworzył urządzenie.
- **Tryb aktualizacji:** `PATHON_UPDATE_GOLDEN=1` tworzy brakujące referencje i nadpisuje różne od obrazu. Bez niego brak referencji jest błędem (test nie tworzy referencji sam, więc CI nie może przejść na „pierwszym uruchomieniu”). Referencje generuje się na lavapipe (CI Linux) albo na sprzęcie dewelopera i ogląda przed commitem; progi `cross_backend()` mają to pokrywać.
- **Niezgodność:** zapis `<nazwa>.actual.png`, `.expected.png` i `.diff.png` do `target/golden-artifacts/` (`PATHON_GOLDEN_ARTIFACTS`), CI wysyła je jako artefakt; panika z liczbą i odsetkiem pikseli, największą różnicą kanału, pierwszym różnym pikselem i ścieżkami. Zmiana rozmiaru to osobny błąd z obiema wielkościami.
- **Brak GPU nie jest zielonym testem po cichu.** `gpu_or_skip` zamienia brak adaptera w jawne pominięcie z komunikatem (w GitHub Actions także adnotacja `::warning::`). Zadania, które muszą mieć GPU (lavapipe, WARP), ustawiają `PATHON_REQUIRE_GPU=1`, więc tam brak adaptera jest porażką. macOS na hostowanych runnerach nie ma gwarancji Metala: zadanie bez `PATHON_REQUIRE_GPU` pomija test z ostrzeżeniem, a nie udaje, że go przeszło. Własny runner z Metalem jest otwartą opcją, gdy ostrzeżenie okaże się stałe.
- **Progi wstępne** (`cross_backend()`: delta 6, 3% pikseli) pochodzą z oceny, nie z pomiaru na wszystkich backendach: referencja pierwszego testu pochodzi z jednego urządzenia, a zgodność lavapipe, WARP i Metal potwierdzi dopiero CI. Po pierwszym przebiegu CI progi albo zostają, albo test dostaje wariant (wpis w tej ADR).

## Konsekwencje

- Test, który przechodzi na GPU dewelopera, może nie przejść na lavapipe: pierwszy przebieg CI dla każdego nowego golden image jest częścią taska, a wariant per backend zapisujemy tu.
- Tolerancja 3% pikseli przepuści niewielkie regresje na krawędziach; testy, które pilnują krawędzi (np. bias cieni), używają `Tolerance::new(…)` z własnym progiem lub `exact()`.
- `png` w grafie zależności deweloperskich (ok. 7 crate'ów); rdzeń i paczki bez zmian.
- Obrazy HDR i formaty inne niż RGBA8 / BGRA8 wymagają rozszerzenia `read_texture` (tonemapping robi M4-09), dopóki testy czytają obraz po tonemappingu, to wystarcza.
