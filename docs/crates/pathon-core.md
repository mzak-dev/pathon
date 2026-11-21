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
4. wszystkie widoki w jednym `submit`, potem `present` okien. Gdy nagrywanie albo `submit` zawiedzie, pobrane klatki okien wracają przez `discard_surface`, więc następna klatka może je pobrać.

Paczka, której `record` zwróci błąd, jest wyłączana z F0105 i znika z kolejności klatki; reszta klatki idzie dalej. Gdy graf się nie skompiluje albo pass zawiedzie, cel wraca w stanie końcowym: wyczyszczony, a obraz hosta (tryb A) nietknięty.

Cache grafu i `ViewId` są per cel, nie per pozycja widoku: zmiana kolejności widoków nie rekompiluje grafów, a odtworzony cel dostaje nowe `ViewId` ([ADR-0010](../adr/0010-tozsamosc-widoku-i-runner-grafu.md)).

Jeden widok na cel w klatce. Drugi widok tego samego celu zachowuje się zależnie od jego rodzaju: offscreen — nadpisuje pierwszy; okno — błąd R0111; zaimportowana tekstura — zły stan wejściowy (zaczyna od `before`, choć pierwszy widok zostawił `after`). Renderer nie wykrywa duplikatów.

`FrameReport::diagnostics` zbiera diagnostyki z tej klatki (F0105, ostrzeżenia grafu). Gdy klatka kończy się błędem, jej diagnostyki trafiają do logu, ale nie do następnego raportu. Po `set_capture(true)` `FrameReport::captures` ma jeden `GraphCapture` na widok, a `feature_details()` opisuje każdą paczkę (status, etap, priorytet, rola, kolejność). Z tego korzystają [devtools](../devtools.md).

## Rejestr paczek

Paczki instaluje się przez `with_pack(pack)` (paczka może wnieść kilka klas i wspólne dane) albo `with_feature(feature)`. Dane dla paczek aplikacja podaje przez `with_shared(value)` i zmienia między klatkami przez `shared_mut()`.

Przy budowie: faza 1 (`requirements` → `DeviceRequest`, konflikt backendów F0201), urządzenie, faza 2 (`negotiate`: F0101 / F0103), kolejność (`sort_nodes` po etapie, `before` / `after`, priorytecie), `setup` w kolejności klatki (F0104) razem z rolami: rolę dostaje pierwsza paczka, której `setup` się udał, kolejne kandydatki są wyłączane z F0102, a paczka, która padnie w `setup`, zostawia rolę następnej. `features()` zwraca status każdej paczki, `active_features()` — aktywne w kolejności klatki.

## Utrata urządzenia (M1-03)

`render_frame` zwraca `RenderError::DeviceLost`. Własne urządzenie: `recover()` (natywnie) albo `recover_async()`. Urządzenie hosta: host tworzy nowe i woła `reset(device)`. Po przebudowie cele offscreen i okna utworzone przez `add_window` wracają same; surface'y i tekstury od hosta trzeba dodać ponownie. Paczki dostają `on_device_reset`, a callback z `on_device_reset(...)` to miejsce na ponowne wgranie danych sceny.

## Wątki

`Renderer: Send`. Core nie tworzy wątków (`xtask deps-check` to sprawdza); runner z wątkiem renderu jest w fasadzie `pathon` (feature `runtime`).
