<div align="center">

# pathon

**Modularny renderer w Rust, który nie wie, kto jest hostem.**

Jeden rdzeń i te same paczki renderujące — we własnym silniku, zamiast renderera Bevy i w Godocie.

[![CI](https://github.com/mzak-dev/pathon/actions/workflows/ci.yml/badge.svg)](https://github.com/mzak-dev/pathon/actions/workflows/ci.yml)
![wgpu 30](https://img.shields.io/badge/wgpu-30-4f6bed)
![Rust 1.99](https://img.shields.io/badge/rust-1.99-orange)
![Licencja](https://img.shields.io/badge/licencja-MIT%20OR%20Apache--2.0-blue)

</div>

---

## Dlaczego

Renderery zwykle zakładają, że to one mają pętlę, okno, wątek renderu i urządzenie GPU. pathon zakłada odwrotnie: `pathon-core` nie ma pętli, okna, wątku, ECS ani systemu assetów. Host go woła, podaje czas i widoki, a urządzenie i cele renderu mogą należeć do hosta ([ADR-0005](docs/adr/0005-hosty-i-tryby-integracji.md)).

Dzięki temu ta sama paczka (SSAO, TAA, FSR…) działa w każdym z trzech trybów:

| Tryb | Co robi pathon | Własny silnik | Bevy | Godot |
| --- | --- | --- | --- | --- |
| **A** — paczki w rendererze hosta | graf pathon w punktach klatki hosta, bufory hosta są slotami | ✅ | opcjonalnie | `CompositorEffect` |
| **B** — scena do tekstury hosta | cała scena na urządzeniu hosta, host składa obraz (np. UI na wierzchu) | ✅ | UI Bevy nad sceną | `Texture2DRD` |
| **C** — jedyny renderer | pathon ma urządzenie i okna | ✅ | Bevy bez `bevy_render` | — |

Generyczność nie jest deklaracją, tylko testem: [`examples/tests/host_seam.rs`](examples/tests/host_seam.rs) renderuje tę samą scenę na urządzeniu utworzonym przez surowy host wgpu (tryb B) i porównuje ją piksel w piksel z trybem C.

## Szybki start

```sh
git clone https://github.com/mzak-dev/pathon.git
cd pathon
cargo run -p pathon-examples --bin vertical_slice      # sześcian w dwóch oknach
cargo run -p pathon-examples --bin host_foreign_device # host ma urządzenie, pathon tylko renderuje
```

### Renderer w kilku linijkach

```rust
use pathon::api::Camera;
use pathon::rhi::{Format, PhysicalSize};
use pathon::{FrameInput, Renderer, ViewInput};

let mut renderer = Renderer::builder().with_feature(Tint).build()?;
let target = renderer.add_offscreen_target(PhysicalSize::new(1280, 720), Format::Rgba8Unorm)?;

renderer.render_frame(&FrameInput {
    views: vec![ViewInput { target, camera: Camera::default() }],
    ..FrameInput::default()
})?;
```

### Własna paczka

Paczka deklaruje passy i ich dostępy do tekstur; graf sam ustala kolejność, usuwa zbędne passy, współdzieli pamięć i wstawia bariery.

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
        let Some(output) = ctx.slot(Slot::Output) else {
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
        ctx.set_slot(Slot::Output, output);
        Ok(())
    }
}
```

### Urządzenie hosta (tryby A i B)

```rust
let adopted = pathon_rhi_wgpu::interop::adopt(instance, adapter, device, queue);
let host_texture = adopted.import_texture("host color", texture)?;
let mut renderer = Renderer::builder().with_device(Arc::new(Device::Wgpu(adopted))).build()?;
let target = renderer.import_texture(host_texture, Access::COLOR_ATTACHMENT, Access::COPY_SRC)?;

renderer.render_frame(&frame)?;               // tryb B: pathon zgłasza na wspólnej kolejce
let commands = renderer.record_frame(&frame)?; // tryb A: host nagrywa do swojego enkodera
```

## Architektura

```mermaid
flowchart TD
    host["Host: własny silnik · Bevy · Godot"]
    facade["pathon (fasada + opcjonalny runner)"]
    core["pathon-core — Renderer"]
    api["pathon-api — kontrakt paczek, sloty"]
    graph["pathon-graph — graf klatki"]
    res["pathon-resources"]
    diag["pathon-diag"]
    rhi["pathon-rhi — fasada RHI"]
    types["pathon-rhi-types"]
    wgpu["pathon-rhi-wgpu"]
    null["pathon-rhi-null (walidacja stanów)"]
    platform["pathon-platform + -winit"]
    packs["paczki (packs/)"]

    host --> facade --> core
    host -. tryby A/B .-> core
    facade -. runtime .-> platform
    packs --> api
    core --> api --> graph --> res
    core --> rhi --> wgpu & null --> types
    graph --> diag
```

Granic pilnuje `cargo xtask deps-check` ([reguły](docs/deps-check.md)): wgpu tylko w `pathon-rhi-wgpu`, winit tylko w `pathon-platform-winit`, paczki widzą tylko `pathon-api`, rdzeń nie tworzy wątków, licencje zależności z [ADR-0001](docs/adr/0001-licencja.md).

| Crate | Rola |
| --- | --- |
| [`pathon-rhi-types`](docs/crates/pathon-rhi-types.md) | formaty, `Caps`, model `Access` per subresource, lista komend, `DeviceApi` |
| [`pathon-rhi-wgpu`](docs/crates/pathon-rhi-wgpu.md) | backend wgpu 30 + `interop` dla hostów i paczek vendorowych |
| [`pathon-rhi-null`](docs/crates/pathon-rhi-null.md) | backend bez GPU, który odrzuca każdy błąd stanu (np. odczyt mipa Hi-Z przed zapisem) |
| [`pathon-graph`](docs/crates/pathon-graph.md) | wersje zasobów, kolejność, culling, aliasing pamięci, plan barier |
| [`pathon-api`](docs/crates/pathon-api.md) | `RenderFeature`, [kontrakt slotów](docs/slots.md), `ViewCtx`, `TaskPool` |
| [`pathon-core`](docs/crates/pathon-core.md) | `Renderer`: rejestr paczek, cele renderu, odzyskiwanie po utracie urządzenia |
| [`pathon-diag`](docs/crates/pathon-diag.md) | diagnostyka w stylu rustc, [strona dla każdego kodu](docs/diag/) |
| [`pathon`](docs/crates/pathon.md) | fasada; feature `runtime` — runner z wątkiem renderu |

## Testy

```sh
cargo test --workspace                     # backend null: każdy plan barier jest walidowany
cargo test --workspace --all-features      # plus prawdziwe GPU: zgodność RHI, wycinek, szew hosta
cargo xtask deps-check                     # granice zależności, licencje, wątki, budżet crate'ów
```

CI uruchamia to na Windows, Linux i macOS, sprawdza kompilację rdzenia na `wasm32` i testy GPU na lavapipe.

## Stan

| Etap | Zakres | Stan |
| --- | --- | --- |
| M0 | ADR, workspace, CI, platforma, RHI v0 | ✅ rdzeń · ⏳ platforma web, testy na Linux i macOS |
| M1 | Caps, diagnostyka, device lost, backend null, RHI v1, test zgodności | ✅ |
| M2 | graf klatki, kontrakt paczek, `Renderer`, pionowy wycinek, szew hosta | ✅ ścieżka ★ · ⏳ cache grafu, historia, profiler |
| M3 | shadery: importy, refleksja, permutacje, kompilacja w tle | ⏳ |
| M4 | PBR, IBL, golden images | ⏳ |
| M5–M7 | paczki (TAA, SSAO, FSR…), Bevy, Godot | ⏳ |

Szczegóły i odhaczone taski: [plan implementacji](docs/plan-implementacji.md). Decyzje: [docs/adr](docs/adr/).

## Licencja

MIT OR Apache-2.0, do wyboru.
