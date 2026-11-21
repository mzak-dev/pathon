# pathon-rhi-wgpu

Backend RHI na wgpu 30 — jedyny crate, który importuje wgpu (M0-08).

- `WgpuDevice::new(&DeviceRequest)` tworzy instancję, adapter i urządzenie. Natywnie `new_blocking` kończy to bez executora, bo natywne wgpu zwraca gotowe futures.
- Wymagane funkcje (`required_features`) bez wsparcia adaptera kończą się `R0002`; opcjonalne włączają się, jeśli adapter je ma. Funkcje eksperymentalne wgpu (ray query, mesh shadery) są na razie wyłączone.
- Konwersje typów pathon ↔ wgpu idą z jednej tabeli na typ (makro `two_way!` dla formatów, tablica par dla funkcji); test sprawdza, że każdy format i każdy bit `Features` przechodzi w obie strony.
- `Caps` liczone z adaptera: matryca formatów pomija formaty, których funkcje (BC, ETC2, ASTC, 16-bit norm) nie zostały włączone.
- Błędy walidacji wgpu przy tworzeniu obiektów i `submit` wracają jako `R0009` zamiast trafiać do handlera, który domyślnie panikuje.
- Komendy `TextureAccess` są pomijane: wgpu śledzi stany sam, plan barier waliduje `pathon-rhi-null`.

## `interop`

Moduł z surowymi obiektami wgpu dla paczek vendorowych i adapterów hostów (`kind = "vendor"` / `"host"`):

- `adopt(instance, adapter, device, queue)` — pathon na urządzeniu hosta (tryby A i B, ADR-0005); handler błędów i callback utraty urządzenia zostają hosta, a utratę host zgłasza przez `Renderer::reset`;
- `raw_device()`, `raw_queue()`, `raw_texture()` — dostęp w drugą stronę;
- `import_texture(label, wgpu::Texture)` / `forget_texture` — tekstura hosta jako cel lub zasób grafu;
- `encode_into(list, &mut encoder)` — lista pathon nagrana do enkodera hosta (tryb A);
- `ExternalContext` — to, co dostaje callback `ExternalCommand` (urządzenie, kolejka, własny enkoder, zadeklarowane tekstury);
- `DeviceHook` — rozszerzenie `DeviceRequest` zmieniające deskryptor urządzenia przed jego utworzeniem.
