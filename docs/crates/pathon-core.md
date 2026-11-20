# pathon-core

`Renderer` — rdzeń silnika (M2-11). Składa klatkę z paczek: rejestruje je, negocjuje z nimi możliwości GPU, ustala kolejność i role, a co klatkę buduje i wykonuje graf z ich passów. Sam nie zna żadnej techniki renderowania.

Nie ma też pętli, okna, wątku, ECS ani systemu assetów (ADR-0005): wołający podaje czas i widoki, a urządzenie i cele renderu mogą należeć do niego.

## Trzy możliwości

| Możliwość | API |
| --- | --- |
| Urządzenie | `Renderer::builder().build()` / `build_async()` tworzy je z wymagań paczek; `with_device(Arc<Device>)` przyjmuje urządzenie hosta (dla wgpu: `pathon_rhi_wgpu::interop::adopt`) |
| Klatka | `render_frame(&FrameInput)` nagrywa, zgłasza i prezentuje okna; `record_frame(&FrameInput)` zwraca `CommandList`, którą host nagrywa do swojego enkodera (`interop::encode_into`) i zgłasza sam |
| Cele | `add_window(SurfaceTarget)` albo `add_surface(SurfaceId, size)` (surface utworzony przez hosta na wątku głównym); `add_offscreen_target(size, format)`; `import_texture(id, before, after)` — tekstura hosta |

Tekstura zaimportowana zachowuje treść hosta (tryb A: paczki dorysowują na obrazie hosta). Okna i cele offscreen są czyszczone kolorem z `with_clear_color` przez pass, który znika, jeśli paczka nadpisze `Output` przez `write`.

## Klatka

Dla każdego widoku z `FrameInput::views`:

1. `prepare_view` aktywnych paczek w kolejności — rola `TemporalResolve` może zmienić `render_size` i jitter;
2. graf: import celu jako slot `Output`, `record` każdej paczki;
3. `compile` + `execute` grafu; błąd grafu → diagnostyka i klatka z samym czyszczeniem (cel nadal da się zaprezentować);
4. wszystkie widoki w jednym `submit`, potem `present` okien.

Paczka, której `record` zwróci błąd, jest wyłączana z F0105; reszta klatki idzie dalej.

## Rejestr paczek

Przy budowie: faza 1 (`requirements` → `DeviceRequest`, konflikt backendów F0201), urządzenie, faza 2 (`negotiate`: F0101 / F0103), kolejność (`sort_nodes` po etapie, `before` / `after`, priorytecie), role (F0102 dla drugiej paczki z tą samą rolą), `setup` (F0104). `features()` zwraca status każdej paczki, `active_features()` — aktywne w kolejności klatki.

## Utrata urządzenia (M1-03)

`render_frame` zwraca `RenderError::DeviceLost`. Własne urządzenie: `recover()` (natywnie) albo `recover_async()`. Urządzenie hosta: host tworzy nowe i woła `reset(device)`. Po przebudowie cele offscreen i okna utworzone przez `add_window` wracają same; surface'y i tekstury od hosta trzeba dodać ponownie. Paczki dostają `on_device_reset`, a callback z `on_device_reset(...)` to miejsce na ponowne wgranie danych sceny.

## Wątki

`Renderer: Send`. Core nie tworzy wątków (`xtask deps-check` to sprawdza); runner z wątkiem renderu jest w fasadzie `pathon` (feature `runtime`).
