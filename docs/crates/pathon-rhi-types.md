# pathon-rhi-types

Wspólne typy wszystkich backendów RHI: formaty, opisy zasobów, `Caps`, model `Access`, lista komend i trait `DeviceApi`. Zależy tylko od `bitflags` i `raw-window-handle` (M0-07).

## Najważniejsze typy

- **Handle'y generacyjne** (`BufferId`, `TextureId`…) i arena `Pool` — backendy trzymają w niej obiekty GPU; handle zwolnionego obiektu nie trafi do nowego obiektu w tym samym slocie.
- **`Caps`** — źródło prawdy o możliwościach adaptera: bity `Features`, `Limits`, matryca formatów. `Profile` (Web / Desktop / High) to tylko nazwany zestaw bitów.
- **`Access` + `SubresourceRange`** — stan zasobu po barierze. Stan łączy dowolne odczyty albo dokładnie jeden rodzaj zapisu (wyjątek: storage read-write). Graf zapisuje plan barier jako `Command::TextureAccess`; backend wgpu je pomija, `pathon-rhi-null` je waliduje.
- **`CommandList`** — nagrywanie niezależne od backendu. `render_pass()` / `compute_pass()` zwracają enkoder, który zamyka pass przy `Drop`. Backend tłumaczy listę przy `submit` albo — w trybach A / B — do enkodera hosta (`interop`).
- **`ExternalCommand`** — callback paczki vendorowej z zadeklarowanymi dostępami; backend podaje mu swój natywny kontekst przez `&mut dyn Any`.
- **`DeviceApi`** — wszystkie metody biorą `&self`, bo urządzenie jest współdzielone między wątkiem głównym (tworzenie surface'ów) a wątkiem renderu (ADR-0004).
- **`MaybeSend` / `MaybeSendSync`** — `Send`/`Sync` natywnie, puste na wasm.

Kody błędów `RhiError::code()` mają strony w `docs/diag/`.
