# ADR-0007: Logowanie

**Status:** Przyjęta
**Data:** 2026-11-20

## Kontekst

Diagnostyka (`pathon-diag`, M1-02) w release idzie jako tekst, a hosty mają własne systemy logów:
- Bevy używa `tracing`, a jego `LogPlugin` przechwytuje też rekordy `log`.
- Godot ma konsolę edytora (`godot_print`, `push_warning`).
- Własny silnik może mieć cokolwiek.

Rdzeń ma budżet zależności (ADR o budżetach w M4-17).

## Decyzja

- Rdzeń i paczki logują przez fasadę **`log`**. Nie instalują loggera; robi to aplikacja albo adapter hosta.
- `tracing` jest tylko za feature w `pathon-profile` (spany passów dla Tracy / puffin, M2-12).
- Adaptery hostów podpinają `log` do hosta:
  - `pathon-bevy` nie robi nic, bo `LogPlugin` Bevy już zbiera `log`;
  - `pathon-godot` instaluje własny `log::Log`, który przekazuje ostrzeżenia i błędy do konsoli Godota.
- Gorąca ścieżka klatki nie loguje. Diagnostyki powtarzalne (np. brak funkcji `Caps` w paczce) są wypisywane raz na kod `pathon-diag`, nie co klatkę.

## Rozważane opcje

| Opcja | Za | Przeciw |
| --- | --- | --- |
| **`log` w rdzeniu, `tracing` w `pathon-profile`** | `log` nie ma zależności; każdy host umie go zebrać | brak spanów i pól strukturalnych w rdzeniu |
| `tracing` wszędzie | spany i pola strukturalne | kilka dodatkowych crate'ów w rdzeniu; spany służą profilowaniu, więc i tak trafiają do `pathon-profile` |

## Konsekwencje

- Kto chce spanów w rdzeniu, włącza `pathon-profile`.
- Brak zainstalowanego loggera = brak logów; przykłady w `examples/` instalują prosty logger.
