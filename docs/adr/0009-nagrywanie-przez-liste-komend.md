# ADR-0009: Nagrywanie przez listę komend

**Status:** Przyjęta
**Data:** 2026-11-20

## Kontekst

Paczki nagrywają komendy do `CommandList` z `pathon-rhi-types`, a backend tłumaczy ją przy zgłoszeniu (M1-05). Daje to walidację stanów na `pathon-rhi-null`, nagrywanie do enkodera hosta (`interop::encode_into`) i niezależność paczek od backendu, ale kosztuje: każda komenda jest raz zapisana do listy i raz odczytana przy tłumaczeniu.

M2-16 wymaga zmierzenia tego narzutu i progu, po którego przekroczeniu backend wgpu przejdzie na nagrywanie bezpośrednie.

## Decyzja

- **Próg:** 15% czasu CPU nagrywania wprost do wgpu, mierzone najszybszym wynikiem benchmarku `examples/benches/recording.rs` przy 1 000 draw callach w jednym passie.
- **Pomiar z 20.11.2026:** +4% przy 1 000 draw callach, +4–6% w całym zakresie 100–10 000 ([docs/bench](../bench/README.md)). Backend wgpu zostaje przy liście komend.
- `Command` ma najwyżej 32 bajty: rzadkie, duże komendy (początek passa, kopie, rozwiązanie zapytań, komendy zewnętrzne) są w `Box`. Bez tego narzut przy 10 000 draw callach wynosił ~31%.
- Jeśli próg zostanie przekroczony, backend wgpu nagrywa komendy od razu podczas `record` paczki. API paczek się nie zmienia: `RenderPassEncoder` pozostaje typem, przez który paczka nagrywa.

## Konsekwencje

- Benchmark trzeba powtórzyć przy zmianach w `Command`, w tłumaczeniu (`encode.rs`) i przy M4-11 (culling na GPU i indirect zmieniają liczbę draw calli na pass).
- Komendy indirect z M4-11 ([ADR-0033](0033-komendy-indirect.md)) są w `Command`, a benchmark powtórzono (wyniki w [docs/bench](../bench/README.md)).
- Limit „`Command` ≤ 32 bajty” pilnuje test `command_stays_within_32_bytes` w `pathon-rhi-types`.
