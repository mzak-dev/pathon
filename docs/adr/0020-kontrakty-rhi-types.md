# ADR-0020: Kontrakty `rhi-types`

**Status:** Przyjęta
**Data:** 2026-11-21

## Kontekst

Spike ash (`docs/spike-ash.md`, pozycje #6 i J) pokazał, że część zachowania RHI wynika dziś tylko z tego, jak działa wgpu: wgpu sam śledzi stany zasobów, jest w pełni wielowątkowy i ma konwencję współrzędnych WebGPU. Natywny backend (ash) musi znać te reguły z kontraktu, a nie zgadywać je z wgpu. Najdroższa do zmiany później jest bariera: `Command::TextureAccess` niesie tylko stan docelowy, więc backend natywny musiałby trzymać mapę stanów per (mip, warstwa) i tłumaczyć listy po kolei — wbrew równoległemu nagrywaniu passów, które ADR-0012 zostawia otwarte.

## Decyzja

### Bariera niesie stan poprzedni

- `Command::TextureAccess { texture, range, from: Access, to: Access }`, `CommandList::texture_access(texture, range, from, to)`. `from` to stan, w którym każdy subresource z `range` jest przed barierą; zakres z różnymi stanami dzieli się na kilka komend.
- `from == Access::empty()` znaczy „zawartość nieważna” (UNDEFINED w Vulkanie): bariera wolno ją porzucić.
- `rhi-null` sprawdza, że `from` równa się śledzonemu stanowi każdego subresource'a z zakresu (R0103); backend wgpu dalej pomija bariery.
- Graf zna stan poprzedni z własnego śledzenia (`StateTracker` w `pathon-graph`), a dla importów z `initial`. Między klatkami stan tekstur z puli pamięta `ResourcePool`, a stan celu offscreen renderer: zapisują go dwufazowo, jak historię — jako oczekujący przy nagrywaniu, zatwierdzony przez `commit_frame` zaraz po udanym `submit`, odrzucony przez `abandon_frame`, gdy lista nie dotarła do GPU (błąd późniejszego widoku albo `submit`); błąd `present` po udanym `submit` niczego nie cofa, bo lista już się wykonała. Dzięki temu `from` zawsze zgadza się ze stanem, w którym GPU naprawdę jest. `rhi-null` jest przy tym atomowe: odrzucona lista nie zmienia żadnego stanu tekstury. Host, który kopiuje z celu offscreen, zwraca go do `SAMPLED`. W trybie A `record_frame` zatwierdza stany przy powrocie: każdą zwróconą listę host musi zgłosić, w kolejności zwrócenia, przed listą następnego `record_frame`; host, który porzuci listę, musi zresetować renderer (`reset`), bo stany puli, historii i celów są już zatwierdzone.
- Limit 32 B na `Command` (ADR-0009): `Access` przechodzi na `u16` (13 bitów zajętych). Gdy zabraknie bitów, wracamy do tego ADR, zanim powiększymy komendę.

### Zapis z hosta (`write_buffer`, `write_texture`)

- Dane są zachowane, a zapisane subresource'y tekstury przechodzą w stan `COPY_DST`: następna bariera ma `from: Access::COPY_DST`. Tak backend natywny nie musi pamiętać uploadu, a bariera z `Access::empty()` po uploadzie jest błędem (R0103) zamiast cichej utraty danych.
- `write_*` jest uporządkowane przed każdym `submit`, który zaczyna się po powrocie `write_*` (z dowolnego wątku).
- `bytes_per_row` w `write_texture` nie ma wymogu wyrównania, musi tylko mieścić wiersz bloków. Kopie bufor↔tekstura w liście komend wymagają `bytes_per_row` podzielnego przez 256 (`COPY_BYTES_PER_ROW_ALIGNMENT` z wgpu). `rows_per_image` musi pokrywać wysokość kopii w blokach zawsze, także dla jednego obrazu (backend wgpu zawsze je przekazuje). `rhi-null` sprawdza te trzy reguły (nowy kod R0113, niepoprawny układ texeli).

### Kolejność w passie

`SetBindGroup` jest dozwolone przed `Set*Pipeline`; grupy zostają po zmianie potoku, jeśli layout na tym indeksie jest zgodny.

### Wątki

- Natywnie każdą metodę `DeviceApi` wolno wołać z dowolnego wątku i współbieżnie z innymi, także `create_*_pipeline` w trakcie `submit` (kompilacja w tle, M3-04). Wyjątek: surface'y tworzy wątek, który ma okno (ADR-0004).
- `destroy_*` zasobu użytego w zgłoszonej liście jest odroczone do końca pracy GPU nad tą listą.
- Na wasm wszystko działa na jednym wątku (`MaybeSend`).

### Współrzędne

Konwencja WebGPU: NDC `x, y ∈ [-1, 1]` z `y` w górę, `z ∈ [0, 1]`; współrzędne framebuffera i tekstur od lewego górnego rogu z `y` w dół; domyślny przód trójkąta CCW (`FrontFace::Ccw`). Backend natywny dopasowuje się. Spike ash (22.11.2026, [spike-ash.md](../spike-ash.md)) narysował trójkąt na Vulkanie i potwierdził, że oba sposoby dają obraz jak w WebGPU (wierzchołek u góry, CCW widoczny, CW odrzucony przy cullingu tylnych): `ADJUST_COORDINATE_SPACE` w naga oraz ujemna wysokość viewportu bez tej flagi. Backend ash ma użyć viewportu z ujemną wysokością (jak wgpu-hal), żeby shadery były identyczne jak na wgpu.

### Bez dynamicznych offsetów

Bind groupy nie mają dynamicznych offsetów uniformów; dane per draw idą przez immediates (ADR-0018) albo osobne grupy.

### Polityka rozrostu struktur

- Do zamrożenia w M5-07 nowe pole w strukturze z publicznymi polami to akceptowana zmiana łamiąca (literały w paczkach i testach).
- Przed M5-07 struktury, które budują paczki i które jeszcze urosną (opisy potoków, layoutów, tekstur, passów), dostają `#[non_exhaustive]` i konstruktor z polami wymaganymi; pola zostają `pub` do odczytu i zmiany po konstrukcji. Nowe pole z wartością domyślną jest wtedy addytywne.
- `Limits`, `Caps` i `AdapterInfo` buduje tylko backend: `#[non_exhaustive]` przed M5-07.
- `DeviceApi` implementują tylko backendy w repo: trait nie należy do zamrożonej powierzchni paczek, nowe metody są dozwolone po M5-07.

### Aneks (22.11.2026): widoki cube i `view_formats`

Warunek wstępny M4-06 (spike-ash #4), bez nowego ADR:

- `TextureDesc::view_formats: &'static [Format]` zamiast `Vec<Format>`, żeby opis został `Copy + Hash` (pooling grafu porównuje opisy przez `same_allocation`, więc tekstury o różnych `view_formats` się nie mieszają). Dozwolony jest tylko bliźniak sRGB / non-sRGB formatu tekstury; to wspólna część wgpu, Vulkana (`VK_KHR_image_format_list`) i WebGPU.
- Cube to nie flaga tekstury: wgpu-hal i Vulkan wnioskują zgodność z cube z kwadratowej tekstury 2D o co najmniej 6 warstwach. Kontrakt siedzi w widoku (`TextureViewDesc::dimension` `Cube` / `CubeArray`), a backend natywny ustawi `CUBE_COMPATIBLE` z `TextureDesc::is_cube_compatible`.
- Bez `layer_count` widok `D1`/`D2`/`D3` obejmuje jedną warstwę, `Cube` sześć, a `D2Array`/`CubeArray` resztę (jak w wgpu); `rhi-null` waliduje to ściśle (R0109 wymiar i format, R0112 zakres).
- Dalej odłożone: `GraphTextureDesc` nie ma cube'a ani `view_formats` (transient w grafie to zawsze 2D z jednym formatem); IBL trzyma mapę środowiska jako trwały zasób (ADR-0011). Import tekstury hosta (`import_texture`) zawsze ma puste `view_formats`.

## Konsekwencje

- Zmiana łamiąca w każdym miejscu, które buduje `TextureAccess` (graf, testy, przykłady); paczki jej nie widzą, bo bariery planuje graf.
- Kod po `write_texture` musi zacząć barierę od `COPY_DST`.
- `rhi-null` łapie błędne `from` i złe `bytes_per_row`, których wgpu albo nie sprawdza, albo zgłosiłby dopiero przy kopiowaniu.
- Mniej wolnych bitów w `Access` (3).
- Tekstura z puli, która ma znany stan, nigdy nie zaczyna klatki od `Access::empty()`, więc backend natywny nie odrzuci jej treści na starcie klatki; `from == empty` służy tylko świeżym teksturom i pierwszemu użyciu.
