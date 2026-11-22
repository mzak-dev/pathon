# Pomiary

Maszyna: AMD Ryzen 5 5600G, AMD Radeon RX 9070 XT (Vulkan), Windows 11 Pro · Rust 1.99.1, profil release · divan 0.1.21, 400 próbek. Pomiar z 20.11.2026.

Dla kodu ograniczonego przez CPU **najszybszy** wynik najmniej zależy od szumu systemu (inne procesy, zegar CPU), więc to on służy do porównań; mediana jest podana obok.

```sh
cargo bench -p pathon-graph --bench compile
cargo bench -p pathon-core --bench frame
cargo bench -p pathon-examples --bench recording   # wymaga GPU
```

Inne pomiary mają własne noty: [cache potoków na dysku (M3-05)](pipeline-cache.md) — test GPU, nie divan.

## Kompilacja grafu klatki (M2-07)

Łańcuch passów postprocessu kończący się w celu renderu (`crates/pathon-graph/benches/compile.rs`).

| Passy | Na zimno (najszybszy / mediana) | Z cache (najszybszy / mediana) | Zysk |
| ---: | --- | --- | ---: |
| 16 | 1,8 µs / 2,9 µs | 0,30 µs / 0,31 µs | ~6× |
| 64 | 6,8 µs / 7,3 µs | 1,2 µs / 1,2 µs | ~6× |
| 256 | 24 µs / 39 µs | 4,5 µs / 7,5 µs | ~5× |

Cache porównuje strukturę grafu dokładnie, zamiast ją hashować. Pierwsza wersja z hashem (SipHash) była **wolniejsza** od kompilacji na zimno (69 µs wobec 42 µs przy 256 passach), bo hashowanie każdego passa kosztuje więcej niż kompilacja.

## Klatka w rdzeniu

`record_frame` z czterema paczkami testowymi (geometria, upscaler na 67%, konsument slotu na żądanie, sześcian unlit) na backendzie null, bez zgłoszenia — czyli `prepare_view`, budowa grafu przez paczki, kompilacja z cache, przydział pamięci z puli i plan barier (`crates/pathon-core/benches/frame.rs`).

| Widoki | Najszybszy | Mediana |
| ---: | --- | --- |
| 1 | 6,1 µs | 10,7 µs |
| 4 | 21,6 µs | 22,1 µs |

## Nagrywanie: lista komend pathon a wgpu wprost (M2-16)

Te same draw calle (bind group, bufor wierzchołków, bufor indeksów, `draw_indexed` na każdy) nagrane wprost do enkodera wgpu albo do `CommandList` pathon i przetłumaczone przez `interop::encode_into`. Oba warianty kończą się na `CommandEncoder::finish`, bez zgłoszenia (`examples/benches/recording.rs`).

| Draw calle | wgpu wprost (najszybszy / mediana) | przez pathon (najszybszy / mediana) | Narzut (najszybszy) |
| ---: | --- | --- | ---: |
| 100 | 52 µs / 90 µs | 56 µs / 87 µs | +6% |
| 1 000 | 418 µs / 689 µs | 437 µs / 703 µs | +4% |
| 10 000 | 4,96 ms / 7,40 ms | 5,16 ms / 6,68 ms | +4% |

Mediany różnią się w granicach szumu w obie strony. Narzut jest poniżej progu z [ADR-0009](../adr/0009-nagrywanie-przez-liste-komend.md) (15%), więc backend wgpu zostaje przy liście komend.

Przed zmniejszeniem `Command` z 80 do 32 bajtów (rzadkie, duże komendy w `Box`) narzut przy 10 000 draw callach wynosił ~31%.
