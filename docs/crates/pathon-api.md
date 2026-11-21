# pathon-api

Kontrakt paczki (M2-10) — wszystko, czym paczka rozszerza silnik. Zwykła paczka zależy tylko od tego crate'u: re-eksportuje RHI (`rhi`), graf (`graph`), narzędzia pamięci (`resources`) i `glam`.

## Punkty rozszerzenia

| Co | Jak | Przykład |
| --- | --- | --- |
| Pass w klatce | `RenderFeature::record` dodaje passy do grafu | SSAO, bloom, tonemapping |
| Tekstura dla innych paczek | slot: wbudowany (`Slot::HDR`…) albo własny `Slot::new("pack.name", format)` | maska obrysu, wektory ruchu wysokiej rozdzielczości |
| Dane dla innych paczek | `Shared` — wartości kluczowane typem, w `SetupCtx`, `PrepareCtx`, `RecordCtx` | bufory sceny, lista świateł, tablica materiałów, ustawienia |
| Wyłączność | rola: wbudowana (`Role::GEOMETRY`…) albo własna `Role::new("sky")` | geometria rasteryzowana vs meshlety, TAA vs FSR |
| Kilka klas naraz | `Pack::register` — paczka instaluje kilka `RenderFeature` i ich wspólne dane | PBR: prepass + oświetlenie + IBL |
| Funkcje GPU | `requirements()` i `negotiate(&Caps)` | ray query z fallbackiem do cieni rastrowych |

Każdy pojedynczy `RenderFeature` jest też `Pack` (implementacja ogólna), więc `with_pack(Tint)` i `with_feature(Tint)` robią to samo.

## `RenderFeature`

| Metoda | Kiedy | Po co |
| --- | --- | --- |
| `requirements()` | przed utworzeniem urządzenia | backend, wymagane i opcjonalne funkcje, rozszerzenia backendu (faza 1) |
| `negotiate(&Caps)` | po utworzeniu urządzenia | `Full`, `Fallback { reason }` (F0103) albo `Disabled { reason }` (F0101) (faza 2) |
| `setup(&mut SetupCtx)` | raz, po negocjacji; po resecie urządzenia paczka wyłączona z F0105 dostaje `negotiate` i `setup` ponownie na tej samej instancji, a jej stare uchwyty GPU są martwe | pipeline'y, bufory, publikacja danych w `Shared` |
| `prepare_view(&mut PrepareCtx)` | co klatkę, przed budową grafu | prośby o sloty na żądanie; rola `TEMPORAL_RESOLVE` ustawia `render_size`, jitter i mip bias |
| `record(&mut RecordCtx)` | co klatkę, dla każdego widoku | passy w grafie, odczyt i zapis slotów i `Shared` |
| `on_resize`, `on_device_reset` | przy zmianie rozmiaru / po utracie urządzenia | domyślnie `on_device_reset` woła `setup` |

Kolejność paczek: `stage()`, potem `before()` / `after()` (nazwy paczek), potem `priority()`; remis rozstrzyga rejestracja z G0006. Ten sam algorytm co dla passów (`sort_nodes`), więc paczki zmieniające ten sam slot dają kolejność niezależną od rejestracji.

`role()` — jedna aktywna paczka na rolę; druga dostaje F0102. `Role::TEMPORAL_RESOLVE` to jedyna rola, którą rdzeń traktuje specjalnie: jej posiadacz ustala rozdzielczość renderu.

## Sloty

`Slot` ma nazwę, format i flagę „na żądanie”; wbudowane sloty i ich konwencje opisuje [docs/slots.md](../slots.md). `RecordCtx::set_slot` odrzuca teksturę w innym formacie (G0005); `create_slot_texture` tworzy ją od razu we właściwym. Sloty na żądanie powstają tylko, gdy konsument poprosi o nie w `prepare_view`.

## `Shared`

Mapa typ → wartość. Paczka publikuje dane w `setup` albo `record`, inne je czytają; aplikacja może podać je przy budowie (`RendererBuilder::with_shared`) i zmieniać między klatkami (`Renderer::shared_mut`). Tak aplikacja przekazuje paczkom scenę bez zależności rdzenia od jakiegokolwiek ECS.

## `ViewCtx`

Rozmiar wyjścia i renderu, jitter, mip bias, kamera (`Camera`, reverse-Z) oraz czas i numer klatki od wołającego — rdzeń nie czyta zegara.

`ViewId` to tożsamość celu widoku: stała, dopóki cel żyje, i nowa po jego ponownym utworzeniu (nie zależy od pozycji widoku w klatce).

## `TaskPool`

ADR-0008: `parallel_for(len, &task)` blokuje do końca, więc zadanie może pożyczać dane klatki. `SerialTaskPool` jest domyślny (także na wasm).

## Domknięcie passa jest `Send`

ADR-0012: domknięcie przekazane do `PassBuilder::record` jest `MaybeSend` (`Send` natywnie, bez wymagań na wasm), żeby passy dało się kiedyś nagrywać równolegle. Paczka nie może więc przechwycić `Rc` ani innego typu bez `Send`:

```rust,compile_fail,E0277
use pathon_api::graph::{PassDesc, Stage};
use pathon_api::{RecordCtx, RenderFeature};
use pathon_api::rhi::RhiError;

struct Pack;

impl RenderFeature for Pack {
    fn name(&self) -> &'static str {
        "rc-pack"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let rc = std::rc::Rc::new(0);
        ctx.graph().add_pass(PassDesc::new("rc", Stage::Opaque)).record(move |_| {
            let _ = &rc;
            Ok(())
        });
        Ok(())
    }
}
```

Kody błędów w `compile_fail` są sprawdzane tylko na nightly; na stable dokumentują zamiar.

## Co widzi paczka

ADR-0012: `pathon_api::graph` re-eksportuje tylko to, co służy do deklarowania passów: `GraphBuilder`, `PassBuilder`, `PassContext`, `PassDesc`, `Stage`, `GraphTextureDesc`, `GraphSize`, `GraphBufferDesc`, `TextureHandle`, `BufferHandle`, `HistoryTexture`. `RecordCtx::graph()` zwraca `GraphBuilder`, który pozwala deklarować zasoby i passy oraz czytać ich opis; kompilacja, wykonanie, cache i pule należą do rdzenia, a rdzeń, devtools i testy grafu korzystają z `pathon-graph` bezpośrednio. `ctx.graph()` zwraca `&mut GraphBuilder`, więc z jednego `let graph = ctx.graph();` da się dodać kilka passów, a `GraphBuilder` można przekazać do funkcji pomocniczej. Konstruktory kontekstów (`SetupCtx::new`, `PrepareCtx::new`, `RecordCtx::new`) mają `#[doc(hidden)]`, co tylko chowa je w dokumentacji; granicą jest to, że paczka nie nazwie `FrameGraph` przez `pathon-api`.

Wnętrze grafu nie jest widoczne dla paczki:

```rust,compile_fail,E0432
use pathon_api::graph::GraphCache;
```

```rust,compile_fail,E0432
use pathon_api::graph::ResourcePool;
```

```rust,compile_fail,E0432
use pathon_api::graph::HistoryStore;
```

```rust,compile_fail,E0432
use pathon_api::graph::FrameGraph;
```

```rust,compile_fail,E0599
use pathon_api::graph::Stage;
use pathon_api::rhi::RhiError;
use pathon_api::{RecordCtx, RenderFeature};

struct Pack;

impl RenderFeature for Pack {
    fn name(&self) -> &'static str {
        "compiling-pack"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let _ = ctx.graph().compile(todo!());
        Ok(())
    }
}
```

Kilka passów z jednego `let graph = ctx.graph();`:

```rust
use pathon_api::graph::{PassDesc, Stage};
use pathon_api::rhi::RhiError;
use pathon_api::{RecordCtx, RenderFeature};

struct Pack;

impl RenderFeature for Pack {
    fn name(&self) -> &'static str {
        "two-passes"
    }

    fn stage(&self) -> Stage {
        Stage::Opaque
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let graph = ctx.graph();
        graph.add_pass(PassDesc::new("first", Stage::Opaque));
        graph.add_pass(PassDesc::new("second", Stage::Opaque));
        Ok(())
    }
}
```
