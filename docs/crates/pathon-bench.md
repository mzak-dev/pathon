# pathon-bench

Wspólny kod pomiarów (M4-17, [ADR-0035](../adr/0035-budzety-i-regresje.md)): protokół, w którym sonda w dowolnym crate'cie zgłasza wynik do `xtask bench`, licznik alokacji i parser `docs/bench/budgets.toml`. To narzędzie deweloperskie (`tools/`), nie wchodzi do builda użytkownika; crate'y renderera używają go tylko w `[dev-dependencies]` (sondy `benches/probe.rs`, testy budżetów).

## Protokół sondy

Sonda to `[[bench]]` z `harness = false`, którego `main` wypisuje na stdout po jednej linii na pomiar (`emit(&metric)`):

```text
PATHON_BENCH<TAB>nazwa<TAB>jednostka<TAB>próbka próbka …
```

Jednostki: `ns` (czas), `bytes`, `count`. Pozostałe linie (komunikaty cargo, własne wydruki) są ignorowane. Nazwa to `<suita>.<obszar>.<pomiar>`, np. `scene.extract.spawn_10k`.

```rust,ignore
use pathon_bench::{CountingAllocator, Metric, count_allocations, emit};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn main() {
    emit(&Metric::timing("moja.suita.czas", &samples));          // ≥ min_samples próbek
    let (_, used) = count_allocations(|| frame());               // licznik na wątek: deterministyczny
    emit(&Metric::count("moja.suita.alokacje", used.allocations));
}
```

Czas mierz wiele razy (próbki to `Duration` jednego przebiegu albo średnia z paczki powtórzeń); liczniki (`count`, `bytes`) zgłaszaj jedną wartością, bo są deterministyczne.

## Budżety

`Budgets::load(path)` czyta `docs/bench/budgets.toml` ([format](../bench/README.md#budgetstoml)); `budgets.max("nazwa")` zwraca `Quantity` w jednostce bazowej (ns, bajty, liczba). Brak wpisu to panika, żeby test nie stracił budżetu bez śladu.
