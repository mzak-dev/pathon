# ADR-0021: Importy shaderów przez wesl

**Status:** Przyjęta
**Data:** 2026-11-22

## Kontekst

M3-01 potrzebuje składania WGSL z modułów, zanim źródło trafi do RHI (ADR-0006: RHI przyjmuje złożony WGSL). Kandydaci z planu: naga_oil (+12 crate'ów) i wesl (+51).

Pierwsza wersja na naga_oil 0.23 nie przeszła recenzji: naga_oil daje `naga::Module`, a zapis z powrotem do WGSL przez `naga::back::wgsl` zmienia nazwy entry pointów i overrides kończących się cyfrą (`pass1`, `scale0`), panikuje na overrides zależnych od innych overrides (`override b = a * 2.0`), a zastąpienie modułu psuje indeksy naga_oil. Każda z tych dróg wymagałaby osobnej łaty.

Spike wesl 0.6.0 (22.11.2026) na tych samych przypadkach: wszystkie składają się do WGSL, który naga 30 parsuje i waliduje — importy przechodnie, overrides i łańcuchy overrides, `var<immediate>`, `enable f16`, `binding_array` (z cechą `naga-ext`), shadery render z grupami 0 i 1 z kilku modułów, `@id(n) override`. Nazwy w shaderze głównym zostają bez zmian. Błędy to `Err`, nigdy panika. wesl nie zależy od naga, działa na `wasm32-unknown-unknown`, licencje mieszczą się w ADR-0001. Bevy 0.20 przeszło na WESL.

## Decyzja

- Crate `pathon-shader` składa moduły przez `wesl` 0.6 (cecha `naga-ext`) z `VirtualResolver` i oddaje **źródło WGSL** z `Compiler::compile_root`. Każde złożenie liczy się od zera z aktualnych modułów.
- Składnia importów to składnia WESL: moduł rejestrowany pod ścieżką `nazwa` jest dostępny jako `import package::nazwa::{element};`; `import` stoi przed `enable`.
- **Nazwy:** deklaracje modułów dostają nazwy z prefiksem ścieżki (`package_colors_pack`); shader główny zachowuje swoje. Override adresowany nazwą (`ShaderStage::constants`, ADR-0018) musi być zadeklarowany w shaderze głównym; override z modułu jest adresowany przez `@id(n)`.
- **Bez usuwania nieużywanego kodu:** shader główny zachowuje wszystkie deklaracje, także nieużywane overrides, żeby klucz potoku (M3-03) nie zależał od tego, co optymalizator wyrzucił.
- Ponowne dodanie modułu pod tą samą ścieżką zastępuje go (pod hot-reload, M3-06).
- `pathon-shader` nie zależy od RHI, wgpu ani naga; łączy go z RHI kod wołający (dziś testy, w M3-03 rejestr shaderów w rdzeniu udostępniony paczkom przez `pathon-api`). Refleksja (M3-02) parsuje złożony WGSL przez naga.
- Błędy składania to `ShaderError` z komunikatem wesl i ścieżką składanego shadera; kody S z lokalizacją są w M3-07.
- wesl jest wpinany przez `PINNED_DEPENDENCIES` w xtask tylko w `pathon-shader`.

## Konsekwencje

- +51 crate'ów w drzewie `pathon-shader` (parser `logos` / `lalrpop-util`, `serde`, `toml`, `annotate-snippets`…); rdzeń ich nie ciągnie, dopóki nie zależy od `pathon-shader`.
- wesl wydaje się mniej więcej co miesiąc i często łamie API w wersjach 0.x — aktualizacja `pathon-shader` przy każdej zmianie wersji.
- Złożony WGSL nie ma komentarzy i jest przeformatowany z AST; błędy backendu wskazują linie po złożeniu — M3-07 musi to mapować.
- Lista słów zarezerwowanych w wesl jest ostrzejsza niż w naga (np. funkcja `get` jest odrzucana).
- Gdy wesl przestanie pasować, wymiana dotyczy tylko `pathon-shader`.
