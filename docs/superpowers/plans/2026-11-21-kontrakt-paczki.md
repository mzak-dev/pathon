# Kontrakt paczki w `pathon-api` — plan wdrożenia

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Cel:** paczka widzi tylko to, czego potrzebuje do deklarowania pracy, a API może rosnąć bez zmian łamiących.

**Architektura:** `GraphBuilder` w `pathon-api` opakowuje `&mut FrameGraph` i wystawia same deklaracje; `pathon_api::graph` re-eksportuje jawną listę; rosnące enumy dostają `#[non_exhaustive]`; domknięcie passa wymaga `MaybeSend`.

**Stack:** Rust 2024, `pathon-rhi-types`, `pathon-graph`, `pathon-api`, `pathon-core`, `pathon-test`, `tools/pathon-devtools`.

**Spec:** [ADR-0012](../../adr/0012-kontrakt-paczki-w-pathon-api.md)

## Ograniczenia globalne

- Wywołania paczek `ctx.graph().<metoda>(...)` kompilują się bez zmian.
- Brak nowych zależności (`xtask deps-check`, baseline 126).
- Kod samokomentujący, komentarze tylko „dlaczego”, docs po polsku.

## Review Focus

1. Paczka nie może zawołać `compile` ani `execute` ani nazwać `GraphCache` / `ResourcePool` / `HistoryStore` przez `pathon-api` — `compile_fail` w Task 2.
2. Domknięcie passa z `Rc` nie kompiluje się natywnie — `compile_fail` w Task 1.
3. Wasm nadal się kompiluje (`MaybeSend` bez `Send` na wasm) — `cargo check --target wasm32-unknown-unknown` w Task 1.
4. Wildcard `_` w dopasowaniach rdzenia nie ukrywa błędów: każde nowe `_ =>` w kodzie rdzenia zwraca błąd albo diagnostykę, a nie cichy fallback — recenzja Task 1.

---

### Task 1: `#[non_exhaustive]` i `MaybeSend` dla passów

**Files:**
- Modify: enumy wymienione w ADR-0012 (`crates/pathon-rhi-types/src/*.rs`, `crates/pathon-graph/src/graph.rs`, `capture.rs`, `crates/pathon-api/src/feature.rs`, `crates/pathon-core/src/registry.rs`); `crates/pathon-graph/src/graph.rs` (`RecordFn`, `PassBuilder::record`); dopasowania w innych crate'ach, które przestaną się kompilować.
- Test: `docs/crates/pathon-api.md` (doctest `compile_fail`).

**Interfaces:**
- Produces: `pub type RecordFn = Box<dyn FnOnce(&mut PassContext<'_>) -> Result<(), RhiError> + MaybeSend>`; `PassBuilder::record(self, record: impl FnOnce(&mut PassContext<'_>) -> Result<(), RhiError> + MaybeSend + 'static)`.

- [ ] **Step 1: Test RED** — w `docs/crates/pathon-api.md` blok ```` ```rust,compile_fail ```` z paczką, której pass przechwytuje `std::rc::Rc` w domknięciu `record` (ma się nie skompilować); uruchom `cargo test -p pathon-api --doc` — blok się kompiluje, więc test pada.
- [ ] **Step 2: Implementacja** `MaybeSend` i `#[non_exhaustive]`; w kodzie poza crate'em enumu dopisz ramiona `_` tylko tam, gdzie kompilator ich wymaga, z sensownym zachowaniem (błąd / `Unsupported` / diagnostyka), nigdy cichym pominięciem.
- [ ] **Step 3: Uruchom** `cargo test --workspace --all-features`, `cargo test -p pathon-api --doc`, `cargo check --target wasm32-unknown-unknown -p pathon-core -p pathon-test -p pathon --no-default-features`, clippy `-D warnings`.
- [ ] **Step 4: Commit** `feat(api)!: require Send pass closures and mark growing enums non_exhaustive (ADR-0012)`

### Task 2: `GraphBuilder` i jawny re-eksport grafu

**Files:**
- Create: `crates/pathon-api/src/graph_builder.rs`
- Modify: `crates/pathon-api/src/lib.rs`, `context.rs`; paczki i dema używające `pathon_api::graph` (`crates/pathon-test/src/*.rs`, `tools/pathon-devtools/src/demo.rs`, testy w `crates/pathon-core/tests/`); `docs/crates/pathon-api.md`.

**Interfaces:**
- Consumes: `PassBuilder::record` z Task 1.
- Produces:
  - `pub struct GraphBuilder<'graph>` z metodami o sygnaturach jak w `FrameGraph`: `create_texture`, `import_texture`, `create_buffer`, `import_buffer`, `history_texture`, `add_pass`, `resource_name`, `texture_format`, `is_latest`.
  - `RecordCtx::graph(&mut self) -> GraphBuilder<'_>`.
  - `pathon_api::graph` = jawna lista z ADR-0012.
  - `#[doc(hidden)]` na `SetupCtx::new`, `PrepareCtx::new`, `RecordCtx::new`.

- [ ] **Step 1: Test RED** — trzy bloki ```` ```rust,compile_fail ```` w `docs/crates/pathon-api.md`: `use pathon_api::graph::GraphCache;`, `use pathon_api::graph::ResourcePool;`, paczka wołająca `ctx.graph().compile(...)`; uruchom `cargo test -p pathon-api --doc` — bloki się kompilują, test pada.
- [ ] **Step 2: Implementacja** według Interfaces; popraw paczki i dema, które nazywały `FrameGraph` przez `pathon-api`.
- [ ] **Step 3: Uruchom** pełny zestaw jak w Task 1.
- [ ] **Step 4: Docs** `docs/crates/pathon-api.md`: krótka sekcja „Co widzi paczka” z listą z ADR-0012.
- [ ] **Step 5: Commit** `feat(api)!: give packs a declaration-only GraphBuilder (ADR-0012)`
