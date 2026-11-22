# `cargo xtask deps-check`

Reguły granic zależności (M0-02), sprawdzane w CI na wyniku `cargo metadata`. Każda reguła ma w `xtask/src/rules.rs` test, który celowo ją łamie.

| Reguła | Co sprawdza |
| --- | --- |
| `pinned-dependency` | `wgpu` tylko w `pathon-rhi-wgpu`, `winit` tylko w `pathon-platform-winit`, `bevy` tylko w `pathon-bevy`, `godot` tylko w `pathon-rhi-godot` i `pathon-godot` |
| `platform-layer` | żaden crate renderera nie zależy od `pathon-platform*` (wyjątek: fasada `pathon` z feature `runtime`) |
| `pack-boundary` | paczka (`packs/` albo `kind = "pack"`) zależy z crate'ów pathon tylko od `pathon-api` i `pathon-shader` ([ADR-0023](adr/0023-permutacje-shaderow.md)); backendu (`interop`) może użyć tylko `kind = "vendor"`, a adapter hosta (`hosts/` albo `kind = "host"`) — backendu i fasady |
| `no-threads` | crate'y w `crates/` nie tworzą wątków (`thread::spawn`, `thread::scope`, `thread::Builder`) — wyjątek: fasada `pathon` (runner) (ADR-0004) |
| `license` | każda zależność ma licencję z listy ADR-0001: MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0, Unlicense, CC0-1.0 (wyrażenia SPDX z `OR`, `AND`, `WITH`) |
| `crate-budget` | domknięcie zależności `pathon-core` nie rośnie o więcej niż 10 crate'ów ponad `CORE_CRATE_BASELINE` (dziś 126, licząc zależności wszystkich platform) |

Rodzaj paczki deklaruje się w jej `Cargo.toml`:

```toml
[package.metadata.pathon]
kind = "vendor"   # albo "pack", "host"
```

Crate'y narzędziowe (`examples/`, `xtask/`) grają rolę hosta albo nie trafiają do użytkownika, więc reguły ich nie dotyczą. MPL-2.0 (gdext) dojdzie do listy dozwolonych razem z `hosts/pathon-godot` (M7).
