# pathon-rhi-types

Wspólne typy wszystkich backendów RHI: formaty, opisy zasobów, `Caps`, model `Access`, lista komend i trait `DeviceApi`. Zależy tylko od `bitflags` i `raw-window-handle` (M0-07).

## Najważniejsze typy

- **Handle'y generacyjne** (`BufferId`, `TextureId`…) i arena `Pool` — backendy trzymają w niej obiekty GPU; handle zwolnionego obiektu nie trafi do nowego obiektu w tym samym slocie.
- **`Caps`** — źródło prawdy o możliwościach adaptera: bity `Features`, `Limits`, matryca formatów. `Profile` (Web / Desktop / High) to tylko nazwany zestaw bitów.
- **Tablice bindingów** — `BindGroupLayoutEntry::count: Option<NonZeroU32>` oznacza stałą tablicę (`binding_array<T, N>`); zasoby dają `BindingResource::TextureViewArray` / `SamplerArray`. Bind groupy są niemutowalne: zmiana elementu to nowa grupa. Reguły i limity (`max_binding_array_elements_per_shader_stage`, `max_binding_array_sampler_elements_per_shader_stage`) opisuje ADR-0017; naruszenia to R0107. Shadery z tablicą wymagają `enable wgpu_binding_array;`.
- **Immediates i overrides** — `Features::IMMEDIATES` i `Limits::max_immediate_size` (profil Web ich nie ma). `RenderPipelineDesc::immediate_size` / `ComputePipelineDesc::immediate_size` (0 = brak) rezerwuje blok widoczny dla wszystkich etapów, a `set_immediates(offset, data)` na enkoderze passa zapisuje do niego dane (`Command::SetImmediates`, dane w `Box<[u8]>`). W WGSL jest to `var<immediate> nazwa: T;` bez dyrektywy `enable`. `ShaderStage::constants` to nazwane stałe `override` z WGSL. Reguły opisuje ADR-0018; w `rhi-null` brak `IMMEDIATES` daje R0002, `immediate_size` ponad limit R0107, a `set_immediates` poza passem, bez potoku, niewyrównane do 4 B albo poza `immediate_size` potoku R0104.
- **`Access` + `SubresourceRange`** — stan zasobu po barierze. Dostęp shaderów dzieli się na `SAMPLED`, `STORAGE_READ` i `STORAGE_WRITE` (ADR-0016). Stan łączy dowolne odczyty albo dokładnie jeden rodzaj zapisu (wyjątek: `STORAGE_WRITE` z `STORAGE_READ`, `DEPTH_WRITE` z `DEPTH_READ`); `is_valid_state` dotyczy buforów, a `is_valid_texture_state` dodatkowo wymaga, by odczyty tekstury należały do jednej klasy układu (`SAMPLED` z `DEPTH_READ`, `STORAGE_READ` z `STORAGE_WRITE`, reszta osobno). Graf zapisuje plan barier jako `Command::TextureAccess`; backend wgpu je pomija, `pathon-rhi-null` je waliduje.
- **`CommandList`** — nagrywanie niezależne od backendu. `render_pass()` / `compute_pass()` zwracają enkoder, który zamyka pass przy `Drop`. Backend tłumaczy listę przy `submit` albo — w trybach A / B — do enkodera hosta (`interop`).
- **`ExternalCommand`** — callback paczki vendorowej z zadeklarowanymi dostępami; backend podaje mu swój natywny kontekst przez `&mut dyn Any`.
- **`DeviceApi`** — wszystkie metody biorą `&self`, bo urządzenie jest współdzielone między wątkiem głównym (tworzenie surface'ów) a wątkiem renderu (ADR-0004).
- **`MaybeSend` / `MaybeSendSync`** — `Send`/`Sync` natywnie, puste na wasm.

Kody błędów `RhiError::code()` mają strony w `docs/diag/`.
