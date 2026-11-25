# ADR-0040: Hot-reload shaderów z dysku

**Status:** Przyjęta
**Data:** 2026-11-25
**Dotyczy:** [ADR-0004](0004-model-watkow.md) (wątki), [ADR-0021](0021-importy-shaderow-wesl.md) (głębokość wesl), [ADR-0023](0023-permutacje-shaderow.md) (unieważnianie kesza), [ADR-0024](0024-kompilacja-w-tle.md) (kompilacja w tle), [ADR-0032](0032-bledy-shaderow-z-lokalizacja.md) (punkty rozszerzeń), M3-06

## Kontekst

M3-06: zmiana `.wgsl` ma być widoczna bez restartu. Do tej pory `ShaderLibrary` i `BackgroundCompiler` umiały podmienić moduł (`add_*_file`, `remove_*`), ale: (1) nikt nie obserwował plików, (2) podmiana wyrzucała **wszystkie** zbudowane permutacje, (3) potoki zbudowane z wyrzuconych modułów zostawały w `PipelineCache` (Linear PAT-123), (4) błędny shader po zapisie odbierałby rysowanie, bo stary moduł znikał, a nowy się nie kompilował, (5) wesl przepełnia stos na patologicznie głębokich wejściach (PAT-122), a pliki użytkownika to dane zewnętrzne, nie kod deweloperski, jak zakładał ADR-0021.

## Decyzja

### Gdzie

`tools/pathon-hot` (nie `crates/`, nie w domknięciu rdzenia): narzędzie deweloperskie z zależnością `notify` 8 (CC0-1.0, dopuszczona przez ADR-0001), tylko natywnie (`cfg(not(target_arch = "wasm32"))`, crate jest na wasm pusty). Wątek własny jest dozwolony, bo reguła `no-threads` dotyczy `crates/` ([deps-check](../deps-check.md)). Paczki nie zależą od `pathon-hot`; host wywołuje `HotReloader::pump` obok `BackgroundCompiler::pump`.

### Przepływ

Wątek `pathon-hot` (stos 64 MiB) trzyma obserwatora i robi wszystko, co wolne: po ciszy `debounce` (60 ms) czyta katalogi od nowa (nie ufa pojedynczym zdarzeniom: zapis atomowy, przeniesienia, usunięcia katalogów i zgubione zdarzenia dają ten sam wynik), mierzy pliki, składa i waliduje zmienione shadery i wysyła wynik kanałem. Wątek klatki (`pump`) tylko przekazuje przyjęte źródła do `BackgroundCompiler` (`add_*_file` / `remove_*`), zwalnia z niego potoki i oddaje listę do zniszczenia. Kompilacja nowych modułów i potoków idzie dalej przez `BackgroundCompiler` na puli.

### Błędny shader nie wyłącza rysowania

Zanim zmiana trafi do biblioteki, wątek obserwatora składa i waliduje (`LibrarySnapshot::check`: wesl + naga, jak `compose_checked`) **każdą permutację, którą biblioteka już zbudowała**, na kopii źródeł z naniesioną zmianą. Jeśli którakolwiek przestaje się kompilować, zmiana nie jest stosowana: biblioteka, moduły i potoki zostają, a log dostaje błąd S01xx z plikiem i linią i K0006. Zmiany zapisane razem są sprawdzane razem (moduł i shader, który go potrzebuje, przechodzą razem); gdy zestaw nie przechodzi, każdy plik jest próbowany osobno w rundach (nie więcej niż 32 plików), więc jeden zepsuty plik nie blokuje drugiego. Zepsuty plik zostaje „oczekujący”: po poprawce dołącza się do następnej zmiany, a ten sam błąd zapisany ponownie nie jest zgłaszany drugi raz.

Sprawdzenie nie obejmuje błędów dopiero przy tworzeniu potoku (S0002, S0110) ani sterownika. Przy nich stare moduły i potoki **nie są oddawane do zniszczenia**: `pump` trzyma je, dopóki żaden build w tle nie zawiódł od ostatniej zmiany i nic nie czeka przez `settle_pumps` (2) kolejnych pumpów. Host rysuje starym potokiem, aż nowy będzie gotowy (wzorzec z `examples/src/hot_quad.rs`: zmienia potok tylko na `Lookup::Ready`).

### Unieważnianie

- `ShaderLibrary` wyrzuca teraz permutacje **tylko tych shaderów, których dotyczy zmiana**: zmieniony shader główny — własne; zmieniony albo usunięty moduł — shadery, które go importują bezpośrednio lub przez inne moduły (ścieżki `package::…` w źródłach, `package_paths`). Gdy któryś moduł używa `super::`, zależności nie da się odczytać z tekstu i wyrzucane jest wszystko (zachowanie sprzed M3-06). Zwracane id modułów jak dotychczas. Trwająca kompilacja shadera, którego zmiana nie dotknęła, nie jest odrzucana (generacja per shader).
- `PipelineCache::evict_modules(&[ShaderModuleId]) -> Vec<Resource>` i `BackgroundCompiler::evict_pipelines` zapominają potoki zbudowane z podanych modułów i oddają je do `release_later`; build potoku z wyrzuconego modułu, który jeszcze trwa, jest niszczony po zakończeniu, a nowy się nie zaczyna (`Lookup::Failed`: zamów klucz shadera od nowa). To zamyka PAT-123.
- `BackgroundCompiler::snapshot` i `ShaderLibrary::snapshot` dają `LibrarySnapshot` (kopia źródeł i lista zbudowanych kluczy) do sprawdzania zmiany na innym wątku. ADR-0032 zapowiadał, że M3-06 nie potrzebuje zmian w `pathon-shader`; potrzebował tych dodatków (nic nie usunięto, `add_*` i `remove_*` mają ten sam podpis).

### Bezpieczeństwo ścieżek i wejścia (PAT-122)

- Czytane są tylko zwykłe pliki `.wgsl` / `.wesl` pod **kanoniczną** ścieżką obserwowanego katalogu. Dowiązania do katalogów (i junctions) nie są śledzone nigdy; dowiązanie do pliku liczy się, gdy jego cel (po `canonicalize`) leży w tym samym katalogu, i jest sprawdzane jeszcze raz przy odczycie. Nazwy pochodzą ze ścieżek względnych, nie z tekstu zewnętrznego, więc `..` nie ma jak się pojawić. K0001.
- Limit rozmiaru pliku (1 MiB), odczyt ucięty na limicie (plik rosnący po sprawdzeniu też odpada), tylko UTF-8, ponowne próby odczytu, gdy edytor trzyma plik. K0002, K0003.
- **Stos wesl.** Pomiar (debug, Windows, wątek 1 MiB / 2 MiB / 64 MiB): zagnieżdżone bloki `{` przepełniają stos od ok. 100 / 200 / ponad 3000 poziomów, łańcuch `a + b + …` od 400 / 800 / ponad 6400 operatorów, nawiasy od 1600 / 3200 / ponad 6400, łańcuch importów od 800 / 1600 / ponad 1600 modułów; czasy składania rosną przy tym szybciej niż liniowo (400 modułów w łańcuchu 2,6 s, 1600 modułów 130 s). Przepełnienie stosu kończy proces, więc `catch_unwind` nie pomaga, a stack nie da się zabezpieczyć samym większym wątkiem (kompilacje nowych permutacji idą na wątkach puli hosta). Dlatego **każdy plik jest mierzony przed kompozycją** (`SourceLimits`: zagnieżdżenie nawiasów 32, operatory w instrukcji 128, głębokość łańcucha importów 32; typowy shader zagnieżdża do ok. 10 poziomów), z zapasem kilku razy względem najgorszego zmierzonego przypadku na 1 MiB. Naruszenie to K0005, plik zostaje przy poprzedniej wersji. Wątek obserwatora ma stos 64 MiB jako druga warstwa dla sprawdzania. Limity są polem `HotConfig`, można je podnieść świadomie.
- Pomiar importów dotyczy też stanu początkowego (pliki obecne przy starcie).

## Rozważane opcje

| Opcja | Za | Przeciw |
| --- | --- | --- |
| **Sprawdzenie zmiany na kopii, potem podmiana** | błąd nie zmienia stanu biblioteki, stare moduły żyją bez żadnego liczenia referencji | dwukrotne składanie przy sukcesie (raz sprawdzenie, raz kompilacja w tle) |
| Podmiana od razu i trzymanie starych modułów do udanej kompilacji | jedno składanie | stan „nowe źródło, stary moduł” w bibliotece; każdy nowy klucz kończy się `Failed` do poprawki |
| `notify-debouncer-full` | gotowy debounce | dodatkowe zależności; i tak czytamy katalog od nowa po ciszy, więc zdarzenia nie muszą być dokładne |
| Zdarzenia notify jako źródło prawdy | mniej odczytów | zapis atomowy, przeniesienia katalogów i zgubione zdarzenia wymagają osobnej obsługi |
| Tylko większy stos zamiast pomiaru wejścia | prostsze | przepełnienie na wątku puli kończy proces; limit 64 MiB też da się przekroczyć |
| Dokładny graf zależności z parsera wesl | pewny | wesl 0.6 nie wystawia grafu importów bez składania; ścieżki `package::` w tekście wystarczają, a `super::` zwija się do „wszystko” |

## Konsekwencje

- Każdy zapis katalogu to pełny odczyt wszystkich plików obserwowanych katalogów. Dla katalogu shaderów (dziesiątki plików) to ułamek milisekundy; dla tysięcy plików to sekundy (limit 32 plików w próbie osobnej też ogranicza koszt zbiorczych zmian).
- Sprawdzenie przed zmianą widzi tylko to, co jest zbudowane. Shader, którego żadna permutacja jeszcze nie powstała, zgłosi błąd dopiero przy pierwszym użyciu (jak bez hot-reloadu).
- `HotReloader::pump` zabiera błędy i ostrzeżenia z `BackgroundCompiler` (`take_failures`, `take_warnings`), żeby je zalogować i zwrócić z resztą; host, który chce je sam, nie woła `pump` hot-reloadu albo czyta je z `HotReport`.
- Paczka, która rysuje potokiem, pyta o niego co klatkę, w której rysuje; to zakłada okno 2 pumpów w `settle_pumps`.
- Przy hot-reloadzie `evicted_modules` w `BackgroundCompiler` rośnie o jeden wpis na wyrzucony moduł do `release_all`.
