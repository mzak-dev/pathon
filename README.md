# pathon

Modularny renderer w Rust na wgpu 30, który da się użyć we własnym silniku, zamiast renderera Bevy i w Godocie. Rdzeń nie wie, kto jest hostem: nie ma pętli, okna, wątku ani ECS ([ADR-0005](docs/adr/0005-hosty-i-tryby-integracji.md)).

## Stan

Gotowy jest generyczny rdzeń (ścieżka ★ z M0–M2): RHI z backendami wgpu i null, diagnostyka, graf klatki, kontrakt paczek, `Renderer`, platforma winit, runner i przykłady. Plan z odhaczonymi taskami: [docs/plan-implementacji.md](docs/plan-implementacji.md).

## Start

```sh
cargo run -p pathon-examples --bin vertical_slice
cargo test --workspace                     # backend null (walidacja stanów)
cargo test --workspace --all-features      # plus testy na prawdziwym GPU
cargo xtask deps-check                     # granice zależności, licencje, wątki
```

## Dokumentacja

- [docs/adr/](docs/adr/) — decyzje (licencja, wątki, hosty, shadery, logowanie, pula wątków)
- [docs/crates/](docs/crates/) — opis każdego crate'u (ta sama treść jest stroną crate'u w rustdoc)
- [docs/slots.md](docs/slots.md) — kontrakt slotów i konwencje
- [docs/diag/](docs/diag/) — strona dla każdego kodu diagnostyki
- [docs/examples.md](docs/examples.md), [docs/deps-check.md](docs/deps-check.md)

Licencja: MIT OR Apache-2.0.
