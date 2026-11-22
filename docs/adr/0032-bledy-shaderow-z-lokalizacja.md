# ADR-0032: Błędy shaderów z lokalizacją w oryginalnym pliku

**Status:** Przyjęta
**Data:** 2026-11-22

## Kontekst

Złożony WGSL (ADR-0021) nie ma komentarzy, jest przeformatowany z drzewa składni, a deklaracje modułów mają nazwy z prefiksem ścieżki (`package_colors_pack`, `_` zamienione na `_<liczba>`). Błąd naga (nieznana nazwa, typy, binding) wskazuje więc linię i kolumnę **złożonego tekstu**, którego developer nigdy nie widział, a błędy wesl nie zawsze mają pozycję. M3-07 wymaga, by błąd w `.wgsl` pokazywał plik, linię i podpowiedź.

wesl 0.6 daje trzy rzeczy: strukturalny błąd z `detail` (źródło, moduł, `Span` bajtowy, nazwa deklaracji), `SourceMap` z mapowaniem nazwy zmanglowanej na `(moduł, nazwa)` (bez pozycji: `span` jest zawsze `None`) i zwykły tekst wyniku. Nie daje mapy linii złożony tekst → oryginał.

## Decyzja

### Diagnostyka

- `ShaderDiagnostic { code, severity, message, location, hint, notes }` w `pathon-shader`, z `Display` w stylu rustc (`error[S0107]: …`, `--> plik:linia:kolumna`, linia źródła z podkreśleniem `^^^`, `= note`, `= hint`, `= docs`) i `to_diagnostic() -> pathon_diag::Diagnostic` (lokalizacja i linia źródła trafiają do komunikatu, więc log pokazuje je bez zmiany typu `Diagnostic`, który nie zna shaderów). `SourceLocation` ma `module`, `file`, `line`, `column`, `length`, `line_text`, `exact`.
- `ShaderError::Compose` i `InvalidWgsl` dostały pole `diagnostic: Box<ShaderDiagnostic>` (`message` zostaje); nowy wariant `Diagnostic` dla S0110; `ShaderError::diagnostic()` zwraca lokalizowaną diagnostykę. `Display` błędu to render rustc.
- **Kody S01xx** ([docs/diag](../diag/S0101.md)): S0101 składnia, S0102 import modułu, którego nie zarejestrowano, S0103 import nazwy, której moduł nie deklaruje, S0104 cykl deklaracji, S0105 niepoprawne `@if`/`@elif`/`@else`, S0106 flaga włączona, ale nieczytana (ostrzeżenie), S0107 nieznana nazwa, S0108 walidacja (typy, wyrażenia), S0109 zasób / binding, S0110 stała potoku bez override, S0111 inne błędy składania. S0001 i S0002 zostają dla błędów bez lokalizacji (nieznany shader główny, błąd RHI, panika). `BuildFailure::code()` i `diagnostic()` zwracają kod i lokalizację S01xx, gdy błąd je ma; `BuildFailure::located()` daje `ShaderDiagnostic`.
- Podpowiedzi: literówka → „did you mean `x`?” (odległość edycji z zamianą sąsiadów jako jedną edycją, próg rośnie z długością nazwy), brak modułu → lista zarejestrowanych, brak nazwy w module → lista jego deklaracji, nieznany override → lista overrides shadera, błąd składni → słowa zamiast terminali parsera.
- **Bez `miette`.** Render rustc to trzydzieści linii i nie wymaga zależności; miette ciągnie kilkadziesiąt crate'ów (backtrace, kolory, wykrywanie terminala), które rdzeń i `pathon-shader` miałyby tylko po to, by narysować jedną linię źródła i podkreślenie. ADR-0007 dalej obowiązuje: biblioteka nie instaluje loggera i nie wypisuje do terminala, host loguje `diagnostic.to_diagnostic()` przez `pathon_diag::emit`. Gdyby host chciał miette, `ShaderDiagnostic` ma wszystko, czego trzeba (plik, offset w `line_text`, długość, hint), a adapter zbuduje `miette::Diagnostic` w kodzie hosta albo `pathon-devtools`.

### Mapowanie złożony tekst → oryginał

wesl nie daje mapy linii, więc `pathon-shader` odtwarza ją przy błędzie (nie w ścieżce bez błędu, więc bez kosztu w normalnym użyciu):

1. **Błędy wesl** niosą `detail.source` i `detail.module_path` (plik) oraz, dla składni i `@if`, `Span`; offset bajtowy zamieniamy na linię i kolumnę. Błędy bez pozycji (nieznany moduł, nieznany element importu, cykl, duplikat) lokalizujemy sami: ścieżka modułu albo nazwa elementu w `import` (wyszukanie całego słowa w źródle pliku wskazanego przez `detail`), deklaracja po nazwie (parsowanie pliku przez `wesl::syntax`, nazwa tokenu w zakresie deklaracji).
2. **Błędy naga** (parse i walidacja) składamy po kompozycji w `compose_checked` (`ShaderLibrary::wgsl`, `BackgroundCompiler`): naga parsuje i waliduje złożony tekst z `Capabilities::all()`, zanim źródło trafi do RHI. Błąd ma zakres bajtów w złożonym tekście; z kilku `SpanContext` bierzemy najwęższy (naga wylicza od funkcji do wyrażenia). Dalej:
   - **Deklaracja:** złożony tekst parsujemy ponownie (`wesl::syntax`) i bierzemy deklarację najwyższego poziomu, która zawiera zakres błędu; jej nazwę odwracamy `EscapeMangler::unmangle` do `(moduł, nazwa)` (brak prefiksu = shader główny).
   - **Pozycja w deklaracji:** wesl drukuje drzewo składni na nowo, więc deklaracja zachowuje **tokeny** (z przemianowanymi identyfikatorami), a zmieniają się odstępy, komentarze i podziały linii. Tokenizujemy złożoną i oryginalną deklarację (bez komentarzy). Gdy liczby tokenów są równe i każda para jest zgodna (interpunkcja identyczna, identyfikator równy po odwróceniu manglingu, liczba z liczbą), token błędu ma ten sam indeks w oryginale. Inaczej szukamy ciągu tokenów błędu w oryginale i przyjmujemy dopasowanie tylko wtedy, gdy ciąg występuje tyle samo razy w obu tekstach (kod usunięty przez `@if` może zawierać te same tokeny; wtedy dopasowanie byłoby zgadywaniem).
   - **Nieodnaleziony token:** lokalizacja wskazuje nazwę deklaracji, `exact = false`, bez podkreślenia, z notką „linia to początek deklaracji”.
   - **Dyrektywy** (`enable`, `requires`, `diagnostic`) leżą poza deklaracjami i wesl zachowuje ich tekst: szukamy tekstu błędu jako całego słowa w plikach.
   - Gdy nic z powyższego nie da pozycji, diagnostyka nie ma lokalizacji, a notka podaje linię i kolumnę w złożonym tekście.
3. **Nazwy w komunikatach** są odwracane (`package_colors_pack` → `colors::pack`), uchwyty arena naga (`[12]`) usuwane, a typy z formatu Debug (`Scalar(Scalar { kind: Float, width: 4 })`) zastępowane nazwami WGSL.
4. **Plik:** `add_root_file` / `add_module_file` (na `ShaderComposer`, `ShaderLibrary`, `BackgroundCompiler`) przyjmują etykietę pliku, którą diagnostyka pokazuje; bez niej jest nazwa shadera głównego albo ścieżka `package::moduł`. Host (hot-reload, M3-06) podaje tu ścieżkę `.wgsl`.

### Kontrola stałych potoku

`check_pipeline_constants(wgsl, &ShaderStage)` porównuje `constants` z overrides złożonego WGSL (nazwa albo numer `@id`); `BackgroundCompiler` woła ją przed `create_*_pipeline` dla modułów z własnej biblioteki. Błąd to S0110 z podpowiedzią (literówka albo lista overrides). Wywołujący własny kod RHI może użyć funkcji sam.

### Ostrzeżenie o fladze (S0106)

Flaga nieużywana przez żadne `@if` to po cichu gałąź `@else` (ADR-0023). Kompozycja zbiera nazwy z `@if`/`@elif` we wszystkich źródłach i ostrzega o włączonej fladze, której nikt nie czyta, z najbliższą czytaną flagą w podpowiedzi. To ostrzeżenie, nie błąd, bo ten sam zestaw flag bywa wspólny dla kilku shaderów. Ostrzeżenia zbiera `ShaderLibrary::take_warnings()` i `BackgroundCompiler::take_warnings()`. Literówki po stronie shadera (`@if(shadw)` przy włączonym `shadows`) wykrywa ta sama kontrola, bo włączony `shadows` jest wtedy nieczytany.

### Punkty rozszerzeń dla M3-06

Zastępowanie modułu i shadera głównego (`add_module`, `add_root`) już unieważnia kesz i podnosi generację (ADR-0024). Doszły `remove_module` i `remove_root` (plik usunięty) w `ShaderComposer`, `ShaderLibrary` i `BackgroundCompiler`, które robią to samo i oddają id usuniętych modułów, oraz etykiety plików. `pathon-hot` nie wymaga dalszych zmian w `pathon-shader`.

## Rozważane opcje

| Opcja | Za | Przeciw |
| --- | --- | --- |
| **Mapa odtwarzana przy błędzie z tokenów deklaracji** | zero kosztu w ścieżce bez błędu, bez forka wesl, dokładna pozycja w typowym przypadku | heurystyka: przy usuniętych gałęziach `@if` bywa tylko deklaracja |
| Komentarze-lokalizatory wstawiane przy składaniu | dokładna mapa linia → linia | wesl usuwa komentarze (drukuje z AST); wymagałoby własnego drukarza albo forka |
| Mapa linii z AST wesl (`Spanned` węzłów po kompozycji) | dokładna | wesl 0.6 nie wystawia pozycji w wyniku; własne drukowanie drzewa to kopia drukarza wesl |
| Walidacja naga dopiero w backendzie (błąd RHI) | brak dodatkowego parsowania | błąd po stronie sterownika bez pliku i linii, różny na backendach |
| `miette` | gotowe renderowanie | zależności, których renderowanie jednej linii nie potrzebuje |

## Konsekwencje

- Koszt: dodatkowe parsowanie i walidacja naga złożonego tekstu przy każdej nowej permutacji (`compose_checked`), na wątku roboczym w `BackgroundCompiler`; RHI parsuje jeszcze raz. Parsowanie naga jest tanie wobec składania wesl (ADR-0021) i kompilacji sterownika. Błąd RHI po przejściu walidacji z `Capabilities::all()` (np. brak funkcji urządzenia) zostaje bez lokalizacji w źródle.
- `ShaderLibrary::wgsl` i `module` dają teraz błąd także dla WGSL, który wesl składa, a naga odrzuca (wcześniej tylko `module` widział to w RHI); testy komponujące niepełne fragmenty używają `ShaderComposer::compose`, które sprawdza wyłącznie to, co wesl.
- **Ograniczenia lokalizacji:** pozycja jest dokładna, gdy tokeny deklaracji da się wyrównać; przy usuniętych gałęziach `@if` z tymi samymi tokenami co błąd wskazujemy nazwę deklaracji. Identyczne tokeny w wielu deklaracjach nie są problemem (deklaracja jest wybierana po nazwie, a pozycja liczona w jej zakresie). Błędy, które naga wykrywa dopiero po przetworzeniu (np. kolizja bindingów na poziomie entry pointu) wskazują zasób, nie entry point. Lokalizacja błędów `override` w pipeline nie istnieje (to opis potoku, nie plik). Pierwszy błąd naga ukrywa kolejne.
- Zmiana zachowania: `BuildFailure::code()` zwraca S01xx zamiast S0001/S0002, gdy błąd ma lokalizację.
- Każdy `ShaderError::Compose` i `InvalidWgsl` zyskał pole (zmiana łamiąca tylko dla kodu, który tworzy te warianty; są `#[non_exhaustive]` na poziomie enum, a wszystkie użycia w repo dopasowują `{ .. }`).
