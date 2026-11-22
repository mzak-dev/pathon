# ADR-0021: Importy shaderów przez wesl

**Status:** Przyjęta
**Data:** 2026-11-22

## Kontekst

M3-01 potrzebuje składania WGSL z modułów, zanim źródło trafi do RHI (ADR-0006: RHI przyjmuje złożony WGSL). Kandydaci z planu: naga_oil (+12 crate'ów) i wesl (+51).

Pierwsza wersja na naga_oil 0.23 nie przeszła recenzji: naga_oil daje `naga::Module`, a zapis z powrotem do WGSL przez `naga::back::wgsl` zmienia nazwy entry pointów i overrides kończących się cyfrą (`pass1`, `scale0`), panikuje na overrides zależnych od innych overrides (`override b = a * 2.0`), a zastąpienie modułu psuje indeksy naga_oil. Każda z tych dróg wymagałaby osobnej łaty.

Spike wesl 0.6.0 (22.11.2026) na tych samych przypadkach: wszystkie składają się do WGSL, który naga 30 parsuje i waliduje — importy przechodnie, overrides i łańcuchy overrides, `var<immediate>`, `enable f16`, `binding_array` (z cechą `naga-ext`), shadery render z grupami 0 i 1 z kilku modułów, `@id(n) override`. Nazwy w shaderze głównym zostają bez zmian. Błędy składni i importów to `Err`; wyjątek to głębokość (patrz Konsekwencje). wesl nie zależy od naga, działa na `wasm32-unknown-unknown`, licencje mieszczą się w ADR-0001. Bevy 0.20 przeszło na WESL.

## Decyzja

- Crate `pathon-shader` składa moduły przez `wesl` 0.6 (cecha `naga-ext`) z `VirtualResolver` i oddaje **źródło WGSL** z `Compiler::compile_root`. Każde złożenie liczy się od zera z aktualnych modułów.
- Składnia importów to składnia WESL: moduł rejestrowany pod ścieżką `nazwa` jest dostępny jako `import package::nazwa::{element};`; `import` stoi przed `enable`.
- **Nazwy:** deklaracje modułów dostają nazwy z prefiksem ścieżki (`package_colors_pack`); shader główny zachowuje swoje. Override adresowany nazwą (`ShaderStage::constants`, ADR-0018) musi być zadeklarowany w shaderze głównym; override z modułu jest adresowany przez `@id(n)`, a identyfikatory `@id` muszą być unikalne w shaderze głównym i we wszystkich modułach naraz (naga odrzuca duplikat dopiero po złożeniu).
- **Usuwanie nieużywanego kodu:** `strip = true` z `keep_main = true`. Shader główny zachowuje wszystkie deklaracje, także nieużywane overrides, żeby klucz potoku (M3-03) nie zależał od tego, co optymalizator wyrzucił; z modułów zostaje tylko to, do czego prowadzi shader główny, więc nieużywane bindingi, overrides i entry pointy modułu nie trafiają do wyniku (refleksja M3-02 widzi tylko to, co shader używa). Override z modułu, którego żadna funkcja nie czyta, znika razem z modułem.
- Ponowne dodanie modułu pod tą samą ścieżką zastępuje go (pod hot-reload, M3-06).
- `pathon-shader` zależy od `pathon-rhi-types` i naga (refleksja, ADR-0006), nie od backendów RHI ani wgpu; z RHI łączy go kod wołający (dziś testy, w M3-03 rejestr shaderów w rdzeniu udostępniony paczkom przez `pathon-api`). Refleksja (M3-02) parsuje złożony WGSL przez naga.
- Błędy składania to `ShaderError` z komunikatem wesl i ścieżką składanego shadera; kody S z lokalizacją są w M3-07.
- wesl jest wpinany przez `PINNED_DEPENDENCIES` w xtask tylko w `pathon-shader`.

## Konsekwencje

- +50 crate'ów (51 z `pathon-shader`) w drzewie `pathon-shader` (parser `logos` / `lalrpop-util`, `serde`, `toml`, `annotate-snippets`…); rdzeń ich nie ciągnie, dopóki nie zależy od `pathon-shader`.
- wesl wydaje się mniej więcej co miesiąc i często łamie API w wersjach 0.x — aktualizacja `pathon-shader` przy każdej zmianie wersji.
- Złożony WGSL nie ma komentarzy i jest przeformatowany z AST; błędy backendu wskazują linie po złożeniu — M3-07 musi to mapować.
- Lista słów zarezerwowanych w wesl jest ostrzejsza niż w naga (np. funkcja `get` jest odrzucana).
- Gdy wesl przestanie pasować, wymiana dotyczy tylko `pathon-shader`.
- **Koszt składania rośnie mniej więcej kwadratowo z liczbą deklaracji** (build release, pomiar z recenzji): 100 funkcji w jednym pliku 37 ms, 300 funkcji 345 ms; łańcuch 300 modułów 225 ms, 1000 modułów 3,8 s; 100 KB trywialnych funkcji 0,36 s, 1 MB 35 s. Shader PBR z kilkuset deklaracjami kosztuje więc dziesiąte części sekundy na permutację, zanim ruszy naga i backend. M3-04 i M3-05 muszą keszować złożone źródła (klucz: shader główny, moduły, definicje), a M3-03 nie może składać w ścieżce klatki.
- **Głębokość:** wesl rekurencyjnie przechodzi AST, więc patologicznie głębokie wyrażenia (łańcuch rzędu tysiąca `+`, kilkaset zagnieżdżonych nawiasów, kilkadziesiąt zagnieżdżonych bloków) albo łańcuchy importów rzędu tysiąca modułów mogą przepełnić stos i przerwać proces; limity są niższe w debugu. Nie dodajemy stacker ani wątku (ADR-0004: wątki tworzy tylko crate `pathon`; wasm). Źródła shaderów to kod deweloperski, nie dane zewnętrzne; gdy M3-06 zacznie ładować pliki użytkownika, guard głębokości trzeba dodać tam.
- wesl nie waliduje typów ani nieznanych rozszerzeń (cecha `eval` wyłączona): `return 1.5` z funkcji `u32` czy `enable bogus;` składają się i padają dopiero w naga, z pozycją w złożonym tekście. Nieużywany import nieznanego modułu jest przyjmowany po cichu.
- Mangling zamienia `_` w nazwach ścieżki i elementu na `_<liczba>` (moduł `cam`, element `unused_tex` → `package_cam__1unused_tex`); M3-07 musi to odwrócić przy mapowaniu nazw.
- Translacja warunkowa (`@if(FOO)`) jest włączona w wesl, ale `pathon-shader` nie ustawia flag, więc gałąź `@else` wybiera się zawsze; flagi to M3-03.
