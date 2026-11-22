# ADR-0008: Pula wątków

**Status:** Przyjęta
**Data:** 2026-11-20

## Kontekst

Core nie tworzy wątków (ADR-0004), a część pracy w klatce warto zrównoleglić: extract, culling na CPU (M4-11), nagrywanie passów. Hosty mają własne pule: Bevy `ComputeTaskPool`, Godot `WorkerThreadPool`, własny silnik — swoją. Druga, niezależna pula obok puli hosta oznacza więcej wątków niż rdzeni i walkę o CPU.

## Decyzja

Trait `TaskPool` w `pathon-api` (core go używa, paczki dostają go w kontekście):

```rust
pub trait TaskPool: MaybeSendSync {
    fn parallel_for(&self, len: usize, task: &(dyn Fn(Range<usize>) + Sync));
    fn thread_count(&self) -> usize;
}
```

- `parallel_for` dzieli `0..len` na zakresy, wykonuje `task` na wątkach puli i wraca dopiero po zakończeniu wszystkich. Dzięki temu `task` może pożyczać dane klatki bez `'static` i `Arc`.
- Implementacje:
  - `SerialTaskPool` w `pathon-api` (wszystko inline) — domyślna, także na wasm;
  - `StdTaskPool` w fasadzie `pathon` (feature `runtime`) na `std::thread::scope`;
  - adaptery hostów: `pathon-bevy` na `ComputeTaskPool::scope`, `pathon-godot` na grupowych zadaniach `WorkerThreadPool`.
- `StdTaskPool` na start tworzy wątki przy każdym wywołaniu. Pułap: koszt startu wątków przy każdym `parallel_for`; zamiana na stałe wątki robocze, gdy pomiar w M4-17 pokaże ten koszt.

## Rozważane opcje

| Opcja | Za | Przeciw |
| --- | --- | --- |
| **`parallel_for` na zakresie indeksów** | da się zaimplementować na każdej puli hosta (Godot ma dokładnie ten model); obiektowo bezpieczny trait | brak zadań asynchronicznych i grafu zależności między zadaniami |
| rayon w rdzeniu | gotowa, szybka pula | własna globalna pula obok puli hosta; kilka dodatkowych crate'ów |
| Executor async (zadania z futures) | elastyczny | za dużo jak na zrównoleglenie pętli w klatce; trudny do zmapowania na `WorkerThreadPool` |

## Konsekwencje

- Bez podanej puli pathon działa szeregowo, więc wasm i testy nie potrzebują wątków.
- Zadania z zależnościami (np. równoległe nagrywanie passów z grafu) układa core w kolejne wywołania `parallel_for`; jeśli to nie wystarczy, wracamy do tego ADR.
- Uzupełnienie ([ADR-0024](0024-kompilacja-w-tle.md)): `TaskPool` ma też `try_spawn` dla zadań, które żyją dłużej niż klatka (kompilacja permutacji w tle); `parallel_for` zostaje bez zmian.
