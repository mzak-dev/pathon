# ADR-0012: Kontrakt paczki w `pathon-api`

**Status:** Przyjęta
**Data:** 2026-11-21

## Kontekst

`pathon-api` re-eksportował `pathon-graph` w całości (`pub use pathon_graph::*`). Paczka widziała więc `GraphCache`, `ResourcePool`, `HistoryStore`, `sort_nodes`, `FrameGraph::compile` i `execute` oraz konstruktory kontekstów, które woła tylko rdzeń. Każda zmiana wnętrza grafu była zmianą API paczek, a M5-07 ma to sprawdzać automatycznie. Do tego domknięcie passa nie musiało być `Send`, co zamykało drogę do równoległego nagrywania passów bez zmiany łamiącej (przegląd architektury z 20.11.2026, kandydat 3).

## Decyzja

- **Paczka dostaje graf przez `GraphBuilder`.** `RecordCtx::graph()` zwraca `GraphBuilder<'_>` z `pathon-api`, który pozwala tylko deklarować: `create_texture`, `import_texture`, `create_buffer`, `import_buffer`, `history_texture`, `add_pass` i odczyty opisu (`resource_name`, `texture_format`, `is_latest`). Kompilacja, wykonanie, cache i pule zostają w rdzeniu.
- **`pathon_api::graph` re-eksportuje jawną listę:** `GraphBuilder`, `PassBuilder`, `PassContext`, `PassDesc`, `Stage`, `GraphTextureDesc`, `GraphSize`, `GraphBufferDesc`, `TextureHandle`, `BufferHandle`, `HistoryTexture`. Rdzeń, devtools i testy grafu korzystają z `pathon-graph` bezpośrednio.
- **Konstruktory kontekstów** (`SetupCtx::new`, `PrepareCtx::new`, `RecordCtx::new`) są `#[doc(hidden)]`: muszą być publiczne, bo woła je inny crate (rdzeń), ale nie są częścią kontraktu paczki.
- **Domknięcie passa jest `MaybeSend`** (`Send` natywnie, bez wymagań na wasm). Równoległe nagrywanie passów będzie można dodać bez zmiany w paczkach.
- **`#[non_exhaustive]` na enumach, które urosną:** `Format`, `Command`, `Transfer`, `RhiError`, `BindingType`, `Resource`, `ResourceRef`, `Backend`, `DeviceType`, `Profile`, `VertexFormat`, `Stage`, `Negotiation`, `FeatureStatus`, `CapturedKind`, `CapturedTarget`. Nowy wariant nie łamie paczek, które dopasowują te enumy.
- `pathon_api::rhi` i `pathon_api::resources` zostają re-eksportem całych crate'ów: `pathon-rhi-types` i `pathon-resources` są projektowane jako kontrakt.

## Rozważane warianty

- **Jawna lista także dla `rhi`:** prawie cały `pathon-rhi-types` jest używany przez paczki; lista byłaby długa i niosła mało ochrony. `#[non_exhaustive]` daje ochronę tam, gdzie API rośnie.
- **Ukrycie konstruktorów kontekstów przez trait w rdzeniu:** więcej kodu tylko po to, by schować trzy funkcje, których paczka i tak nie ma po co wołać.
- **`cargo-semver-checks` w CI teraz:** potrzebuje wersji bazowej (crates.io albo tag); zostaje w M5-07.

## Konsekwencje

- Paczki zmieniają się tylko tam, gdzie nazywały typ `FrameGraph` albo dopasowywały enum bez `_`; wywołania `ctx.graph().add_pass(...)` zostają bez zmian.
- Domknięcie passa nie może trzymać `Rc` ani innych typów bez `Send` (natywnie).
- Testy `compile_fail` w dokumentacji `pathon-api` pilnują, że wnętrze grafu nie jest widoczne dla paczek.
