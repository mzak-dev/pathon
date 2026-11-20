<div align="center">

# pathon

**Ultra-modularny i wydajny silnik renderowania w Rust.**

Rdzeń robi tylko to, czego potrzebuje każda klatka. Wszystko, co rysuje — geometria, światło, cienie, antyaliasing, upscaling, postprocess — to paczki.

[![CI](https://github.com/mzak-dev/pathon/actions/workflows/ci.yml/badge.svg)](https://github.com/mzak-dev/pathon/actions/workflows/ci.yml)
![wgpu 30](https://img.shields.io/badge/wgpu-30-4f6bed)
![Rust 1.99](https://img.shields.io/badge/rust-1.99-orange)
![Licencja](https://img.shields.io/badge/licencja-MIT%20OR%20Apache--2.0-blue)

</div>

---

## Rdzeń i paczki

```mermaid
flowchart LR
    subgraph packs["Paczki — rozszerzają silnik"]
        direction TB
        geo["geometria / PBR"]
        light["światło, cienie, IBL"]
        temporal["TAA, FSR"]
        post["SSAO, SSR, bloom, tonemapping"]
        yours["twoja paczka"]
    end
    subgraph core["Rdzeń — mały i stały"]
        direction TB
        registry["rejestr paczek: wymagania, negocjacja, role, kolejność"]
        graph["graf klatki: culling, aliasing pamięci, bariery"]
        rhi["RHI: wgpu 30 · null (walidacja)"]
    end
    packs -- "pathon-api" --> core
```

**Rdzeń** składa klatkę z tego, co zadeklarują paczki: ustala kolejność, usuwa zbędną pracę, przydziela pamięć i wstawia bariery. Nie zna żadnej techniki renderowania.

**Paczka** to zwykły crate zależny tylko od `pathon-api`. Deklaruje, czego potrzebuje od GPU, i dokłada passy do grafu klatki. Nowa paczka nie wymaga zmiany w rdzeniu — to reguła projektu, pilnowana przez [paczki testowe](docs/crates/pathon-test.md) i `cargo xtask deps-check`.

## Modularność

- **Kontrakt w jednym crate'cie.** `RenderFeature`: `requirements()` przed utworzeniem urządzenia, `negotiate(&Caps)` po nim (pełna ścieżka, fallback albo wyłączenie z diagnostyką), `setup`, `prepare_view`, `record` ([pathon-api](docs/crates/pathon-api.md)).
- **Sloty zamiast zależności między paczkami.** Paczki wymieniają się teksturami przez nazwane sloty o stałym [kontrakcie formatów i konwencji](docs/slots.md) — wbudowane (`Slot::DEPTH`, `Slot::NORMALS`, `Slot::HDR`, `Slot::OUTPUT`…) albo własne (`Slot::new("outline.mask", Format::R8Unorm)`). SSAO nie wie, kto narysował głębię.
- **Dane współdzielone.** Bufory sceny, listy świateł czy ustawienia przechodzą między paczkami — i od aplikacji do paczek — przez `Shared`, kluczowane typem. Paczki nie zależą od swoich crate'ów nawzajem.
- **Role.** Jedna aktywna paczka na rolę — wbudowaną (geometria, rozdzielczość temporalna, tonemapping) albo własną (`Role::new("sky")`). Podmiana TAA na FSR albo geometrii rastrowej na meshlety to zmiana jednej paczki.
- **Paczka to zestaw.** `Pack` instaluje naraz kilka klas renderujących razem z ich wspólnymi danymi (np. PBR: prepass + oświetlenie + IBL); pojedynczy `RenderFeature` też jest paczką.
- **Kolejność bez kolejności rejestracji.** Etap, `before` / `after`, priorytet; remis kończy się ostrzeżeniem, nie losowym wynikiem.
- **Caps zamiast tierów.** Paczka prosi o konkretne bity (`SHADER_INT64_ATOMIC_MIN_MAX`, `RAY_QUERY`…) i sama decyduje o fallbacku na słabszym sprzęcie.
- **Wymienne zaplecze.** RHI to osobna warstwa z backendem wgpu i backendem null; kolejne backendy (ash, Metal, Godot) nie zmieniają paczek.

## Wydajność

- **Brak pracy, której nikt nie chce.** Graf usuwa passy bez odbiorców; slot, którego nikt nie czyta, kosztuje zero. Pass nadpisany w całości przez kolejny znika sam.
- **Pamięć współdzielona w klatce.** Tekstury tymczasowe o tym samym kształcie i rozłącznych czasach życia dzielą jedną teksturę fizyczną.
- **Zero alokacji GPU w stabilnej scenie.** Pula tekstur trzyma zasoby między klatkami; test sprawdza, że po pierwszej klatce nic nowego nie powstaje.
- **Bariery liczone, nie pisane ręcznie.** Plan barier per mip i warstwa wynika z deklaracji passów; backend null odrzuca każdy błąd stanu w testach.
- **Jedno zgłoszenie na klatkę**, wszystkie widoki w jednej liście komend.
- **Mały rdzeń.** Domknięcie zależności `pathon-core` to 126 crate'ów (z wgpu) i nie rośnie bez uzasadnienia — pilnuje tego CI.

Pomiary narzutu (benchmark nagrywania M2-16, budżety klatki M4-17) są w planie; liczby trafią tu, gdy będą.

## Twoja paczka

```rust
use pathon::api::graph::{PassDesc, Stage};
use pathon::api::rhi::{Access, Color, ColorAttachment, RenderPassDesc, RhiError};
use pathon::api::{RecordCtx, RenderFeature, Slot};

struct Tint;

impl RenderFeature for Tint {
    fn name(&self) -> &'static str {
        "tint"
    }

    fn stage(&self) -> Stage {
        Stage::PostProcess
    }

    fn record(&mut self, ctx: &mut RecordCtx<'_>) -> Result<(), RhiError> {
        let Some(output) = ctx.slot(Slot::OUTPUT) else {
            return Ok(());
        };
        let mut pass = ctx.graph().add_pass(PassDesc::new("tint", Stage::PostProcess));
        let output = pass.write(output, Access::COLOR_ATTACHMENT);
        pass.record(move |pass| {
            let view = pass.view(output)?;
            drop(pass.commands().render_pass(RenderPassDesc {
                label: "tint",
                color: vec![ColorAttachment::clear(view, Color::rgba(0.2, 0.1, 0.3, 1.0))],
                depth: None,
                timestamps: None,
            }));
            Ok(())
        });
        ctx.set_slot(Slot::OUTPUT, output);
        Ok(())
    }
}
```

```rust
let mut renderer = Renderer::builder().with_pack(Tint).build()?;
let target = renderer.add_offscreen_target(PhysicalSize::new(1280, 720), Format::Rgba8Unorm)?;
renderer.render_frame(&FrameInput {
    views: vec![ViewInput { target, camera: Camera::default() }],
    ..FrameInput::default()
})?;
```

`Tint` zapisuje `Output` w całości, więc domyślne czyszczenie celu znika z grafu — rdzeń nie wykonuje pracy, której wynik i tak zostałby nadpisany.

## Szybki start

```sh
git clone https://github.com/mzak-dev/pathon.git
cd pathon
cargo run -p pathon-examples --bin vertical_slice   # sześcian przez cały stos, dwa okna
```

## Gdzie działa

Rdzeń nie ma własnej pętli, okna, wątku ani ECS, więc ten sam silnik i te same paczki działają w różnych miejscach ([ADR-0005](docs/adr/0005-hosty-i-tryby-integracji.md)):

| | Jak |
| --- | --- |
| Samodzielnie | runner z fasady `pathon`: okna na wątku głównym, render na osobnym wątku |
| We własnym silniku | silnik woła `render_frame`, podaje swoje okno albo swoje urządzenie wgpu |
| W Bevy (plan: M6) | zamiast `bevy_render` albo pod UI Bevy na wspólnym urządzeniu |
| W Godocie (plan: M7) | paczki w `CompositorEffect` albo scena pathon w `Texture2DRD` |

## Crate'y

| Crate | Rola |
| --- | --- |
| [`pathon`](docs/crates/pathon.md) | fasada; feature `runtime` — runner z wątkiem renderu |
| [`pathon-core`](docs/crates/pathon-core.md) | `Renderer`: rejestr paczek, cele renderu, odzyskiwanie po utracie urządzenia |
| [`pathon-api`](docs/crates/pathon-api.md) | kontrakt paczek: `RenderFeature`, sloty, `ViewCtx`, `TaskPool` |
| [`pathon-graph`](docs/crates/pathon-graph.md) | graf klatki: wersje, kolejność, culling, aliasing, bariery |
| [`pathon-resources`](docs/crates/pathon-resources.md) | `GpuArray`, ring uniformów, sub-alokacja, opóźnione zwalnianie |
| [`pathon-rhi`](docs/crates/pathon-rhi.md) / [`-types`](docs/crates/pathon-rhi-types.md) | warstwa sprzętowa: typy, `Caps`, lista komend |
| [`pathon-rhi-wgpu`](docs/crates/pathon-rhi-wgpu.md) | backend wgpu 30 + `interop` dla paczek vendorowych |
| [`pathon-rhi-null`](docs/crates/pathon-rhi-null.md) | backend bez GPU, waliduje stany zasobów |
| [`pathon-diag`](docs/crates/pathon-diag.md) | diagnostyka z kodem i [stroną dla każdego kodu](docs/diag/) |

## Testy

```sh
cargo test --workspace                  # backend null: każdy plan barier walidowany
cargo test --workspace --all-features   # plus GPU: zgodność RHI, pionowy wycinek, szew hosta
cargo xtask deps-check                  # granice paczek i warstw, licencje, wątki, budżet crate'ów
```

CI: Windows, Linux, macOS, `wasm32` i testy GPU na lavapipe.

## Stan

| Etap | Zakres | Stan |
| --- | --- | --- |
| M0–M2 | RHI, diagnostyka, graf klatki, kontrakt paczek, `Renderer`, platforma, runner | ✅ ścieżka ★ |
| M3 | shadery: importy, refleksja, permutacje, kompilacja w tle | ⏳ |
| M4 | PBR, IBL, cienie, postprocess, golden images | ⏳ |
| M5 | paczki: TAA, SSAO, FSR 1, SSS, SSR, FXAA, picking | ⏳ |
| M6–M9 | Bevy, Godot, meshlety, RT, FSR 4, backendy natywne | ⏳ |

Szczegóły: [plan implementacji](docs/plan-implementacji.md) · decyzje: [docs/adr](docs/adr/).

## Licencja

MIT OR Apache-2.0, do wyboru.
