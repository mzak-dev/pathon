# pathon-rhi-wgpu

Backend RHI na wgpu 30 — jedyny crate, który importuje wgpu (M0-08).

- `WgpuDevice::new(&DeviceRequest)` tworzy instancję, adapter i urządzenie. Natywnie `new_blocking` kończy to bez executora, bo natywne wgpu zwraca gotowe futures.
- Wymagane funkcje (`required_features`) bez wsparcia adaptera kończą się `R0002`; opcjonalne włączają się, jeśli adapter je ma. Funkcje eksperymentalne wgpu (ray query, mesh shadery) są na razie wyłączone.
- Konwersje typów pathon ↔ wgpu idą z jednej tabeli na typ (makro `two_way!` dla formatów, tablica par dla funkcji); test sprawdza, że każdy format i każdy bit `Features` przechodzi w obie strony.
- `Caps` liczone z adaptera: matryca formatów pomija formaty, których funkcje (BC, ETC2, ASTC, 16-bit norm) nie zostały włączone.
- Błędy walidacji wgpu przy tworzeniu obiektów i `submit` wracają jako `R0009` zamiast trafiać do handlera, który domyślnie panikuje.
- Przy włączonym `Features::PIPELINE_CACHE` (dziś Vulkan) urządzenie ma jeden `wgpu::PipelineCache` przekazywany każdemu potokowi. Własne urządzenie startuje z blobem z `DeviceRequest::pipeline_cache`; cache powstaje w zakresie błędów walidacji, a blob odrzucony przez wgpu (inny adapter lub backend, uszkodzony, ze starszego sterownika) daje pusty cache, nigdy błąd ani panikę, urządzenie hosta (`adopt`) dostaje cache bez danych; `pipeline_cache_data()` oddaje blob do zapisu (ADR-0019).
- Domyślny widok tekstury (`TextureViewDesc::default()`) bierze format z opisu tekstury, gdy ten różni się od formatu zasobu wgpu: powierzchnie webowe są skonfigurowane jako Unorm, a opisane jako sRGB, i bez tego kodowanie sRGB ginęłoby w widoku (kolor w Chrome różniłby się od natywnego).
- Test `tests/caps_report.rs` (`gpu-tests`) porównuje `Caps` z surowym wgpu na każdym natywnym backendzie systemu, dla urządzenia pathon i przyjętego przez `adopt`, i zapisuje raport do pliku z `PATHON_CAPS_REPORT` (CI: podsumowanie joba).
- Klatka powierzchni żyje do `present_surface`/`discard_surface`: wtedy backend zwalnia jej widok i każdy widok utworzony z jej tekstury, żeby `configure_surface` zastał zero uchwytów do bufora zwrotnego (DX12 `ResizeBuffers` inaczej zawodzi, [ADR-0034](../adr/0034-wyjscie-hdr.md)).
- Powierzchnie: `surface_caps` składa `SurfaceCaps` z `get_capabilities().format_capabilities` (formaty i przestrzenie barw, tylko formaty znane pathonowi) i `display_hdr_info` (nity, headroom). `configure_surface` zawsze przekazuje wgpu jawną przestrzeń (nigdy `Auto` wgpu, które dla `Rgba16Float` wybrałoby scRGB); wybór robi `SurfaceCaps::resolve` z `pathon-rhi-types` ([ADR-0034](../adr/0034-wyjscie-hdr.md)). `display_hdr_info` na Metalu działa tylko na głównym wątku.
- Komendy `TextureAccess` są pomijane: wgpu śledzi stany sam, plan barier waliduje `pathon-rhi-null`.

## `interop`

Moduł z surowymi obiektami wgpu dla paczek vendorowych i adapterów hostów (`kind = "vendor"` / `"host"`):

- `adopt(instance, adapter, device, queue)` — pathon na urządzeniu hosta (tryby A i B, ADR-0005); handler błędów i callback utraty urządzenia zostają hosta, a utratę host zgłasza przez `Renderer::reset`;
- `raw_device()`, `raw_queue()`, `raw_texture()` — dostęp w drugą stronę;
- `import_texture(label, wgpu::Texture)` / `forget_texture` — tekstura hosta jako cel lub zasób grafu;
- `encode_into(list, &mut encoder)` — lista pathon nagrana do enkodera hosta (tryb A);
- `ExternalContext` — to, co dostaje callback `ExternalCommand` (urządzenie, kolejka, własny enkoder, zadeklarowane tekstury);
- `DeviceHook` — rozszerzenie `DeviceRequest` zmieniające deskryptor urządzenia przed jego utworzeniem.
