# ADR-0002: Nazwa i publikacja

**Status:** Proponowana (czeka na rezerwację nazwy)
**Data:** 2026-11-20

## Kontekst

crates.io nie ma przestrzeni nazw: kto pierwszy opublikuje crate o danej nazwie, ten ją ma. 20.11.2026 nazwa `pathon` jest wolna (API crates.io zwraca 404).

## Decyzja

- Projekt nazywa się **pathon**; fasada to crate `pathon`, pozostałe crate'y mają prefiks `pathon-` (`pathon-core`, `pathon-rhi-wgpu`, `pathon-bevy`…).
- Nazwę `pathon` rezerwuje autor ze swojego konta: publikuje fasadę `0.0.1` z README wskazującym repo, gdy tylko istnieje workspace z M0-01.
- Pozostałe crate'y idą na crates.io dopiero, gdy mają działający kod. Rezerwowanie kilkudziesięciu pustych nazw łamie zasady crates.io dotyczące zajmowania nazw.

## Konsekwencje

- Ryzyko: ktoś może zająć np. `pathon-core` przed publikacją. Jest niskie, a w razie czego zmieniamy nazwę jednego crate'u; fasada `pathon` zostaje.
- Wersje: wszystkie crate'y workspace'u mają wspólny numer wersji (`[workspace.package] version`), dopóki API nie jest stabilne.

## Do zrobienia

1. [ ] Autor: opublikować pusty crate `pathon` 0.0.1 (README wskazujące repo). Fasada z workspace'u nie nadaje się do tego, dopóki jej zależności nie są na crates.io (`cargo publish --dry-run -p pathon`: „dependency `pathon-api` does not specify a version”); kroki w [weryfikacji platform](../weryfikacja-platform.md). Stan 22.11.2026: nazwa nadal wolna.
