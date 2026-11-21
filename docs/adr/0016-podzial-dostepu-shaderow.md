# ADR-0016: Podział dostępu shaderów na sampled i storage

**Status:** Przyjęta
**Data:** 2026-11-21

## Kontekst

`Access::SHADER_READ` / `SHADER_WRITE` nie rozróżniały próbkowania od dostępu storage. Natywny backend (spike ash, `docs/spike-ash.md`, pozycja #2) potrzebuje tego rozróżnienia, bo tekstura próbkowana i tekstura storage mają różne układy pamięci, a układ deskryptora jest ustalany przy `create_bind_group`, zanim znany jest jakikolwiek `Access`. Do tego `usage_for` w `pathon-graph` mapował `SHADER_READ` tylko na `TextureUsage::SAMPLED`, więc pass czytający teksturę storage bez passa, który ją zapisuje, dostawał teksturę bez `STORAGE` — błąd na wgpu już dziś.

## Decyzja

- `SHADER_READ` i `SHADER_WRITE` znikają. W ich miejsce: **`SAMPLED`** (próbkowanie / odczyt przez widok tekstury), **`STORAGE_READ`**, **`STORAGE_WRITE`** (tekstury i bufory storage). `SHADER_READ` nie zostaje jako suma.
- **Stan tekstury** łączy tylko odczyty o tym samym układzie: `SAMPLED` z `DEPTH_READ`; `STORAGE_READ` z `STORAGE_WRITE`. `SAMPLED | COPY_SRC`, `SAMPLED | STORAGE_READ` i podobne mieszanki układów są błędem (`Access::is_valid_texture_state`). Bufory nie mają układów, więc ich odczyty łączą się dowolnie (`Access::is_valid_state` jak dotąd: najwyżej jeden rodzaj zapisu; `STORAGE_WRITE` może iść z `STORAGE_READ`, `DEPTH_WRITE` z `DEPTH_READ`).
- **Użycia w grafie:** `SAMPLED` → `TextureUsage::SAMPLED`; `STORAGE_READ` / `STORAGE_WRITE` → `TextureUsage::STORAGE` i `BufferUsage::STORAGE`.
- **Stan końcowy celów offscreen** to `SAMPLED` (dotąd `SHADER_READ | COPY_SRC`). Host, który kopiuje z celu, sam wstawia przejście do `COPY_SRC` (na wgpu bariery są niejawne).
- **Walidacja w `rhi-null`:** binding próbkowanej tekstury wymaga `SAMPLED` (dla formatu głębi także `DEPTH_READ`), tekstury storage — `STORAGE_READ` i/lub `STORAGE_WRITE` zgodnie z trybem dostępu, bufora storage — `STORAGE_*`, uniform buffera — `UNIFORM`.

## Konsekwencje

- Zmiana łamiąca dla paczek: każde `SHADER_READ` / `SHADER_WRITE` w deklaracjach dostępu trzeba zamienić. Przed M5-07 to tanie; po zamrożeniu kontraktu byłoby drogie.
- Układ deskryptora w przyszłym backendzie natywnym wynika z typu bindingu i formatu, a nie z bitów stanu.
