# Cache potoków na dysku (M3-05)

Pomiar z 22.11.2026. Maszyna: AMD Ryzen 5 5600G, AMD Radeon RX 9070 XT (Vulkan, sterownik AMD proprietary 26.11.2, LLPC), Windows 11 Pro · Rust 1.99.1, profil dev (czas liczy sterownik, nie nasz kod) · wgpu 30, `Features::PIPELINE_CACHE`. Komputer był w tym czasie zajęty innymi budowami, więc bezwzględne wartości skaczą (±40%); kierunek i rzędy wielkości są stabilne.

```sh
cargo test -p pathon-shader --features gpu-tests --test disk_cache_gpu -- --nocapture the_second
PATHON_KEEP_DRIVER_CACHES=1 cargo test -p pathon-shader --features gpu-tests --test disk_cache_gpu -- --nocapture the_second
```

## Co mierzy test

`crates/pathon-shader/tests/disk_cache_gpu.rs`: 8 shaderów compute po 400 operacji (sin, cos, fma; sól w kodzie, żeby sterownik nie znał ich z poprzednich uruchomień). Każdy „start” to **osobny proces** (to samo binarium testowe, wejście `start_in_this_process`): tworzy urządzenie Vulkan (z blobem z `PersistentPipelineCache::load`, gdy plik istnieje), tworzy moduły, **mierzy tylko `create_compute_pipeline` ośmiu potoków**, zapisuje cache (`save`). Trzy starty tych samych shaderów:

1. pierwszy start: pusty cache, zapis pliku;
2. drugi start: nowy proces, nowe urządzenie, cache z pliku;
3. kontrola: nowy proces, bez pliku (czas kompilacji bez pomocy blobu).

## Wyniki

### Cache sterownika wyłączony (domyślny tryb testu)

Sterownik AMD ma własny cache shaderów, który przeżywa urządzenie i proces i leży poza katalogami użytkownika. Bez wyłączenia go (zmienna `AMD_VK_USE_PIPELINE_CACHE=0` w procesach potomnych, razem z pustymi `LOCALAPPDATA`, `XDG_CACHE_HOME`, `MESA_SHADER_CACHE_DIR`) kontrola jest równie szybka jak drugi start i wpływu naszego blobu nie widać (niżej). Z wyłączonym cache sterownika, 5 uruchomień:

| Start | Czas 8 potoków |
| --- | ---: |
| pierwszy (pusty cache) | 2,7–4,6 s |
| drugi (cache z pliku, 215 kB) | **0,63–1,05 s** |
| kontrola (bez cache) | 1,9–5,2 s |

Drugi start jest **3,5–5,7 razy szybszy od pierwszego** i **3–7 razy szybszy od kontroli** (wszystkie 5 uruchomień zielone, próg testu: 1,25×). Blob 215 kB dla 8 potoków.

### Cache sterownika włączony (`PATHON_KEEP_DRIVER_CACHES=1`)

| Start | Czas 8 potoków |
| --- | ---: |
| pierwszy (pusty cache) | 2,9–4,7 s |
| drugi (cache z pliku) | 0,86–1,0 s |
| kontrola (bez cache) | 0,79–0,85 s |

Drugi start też jest 3–5 razy szybszy od pierwszego, ale kontrola jest tak samo szybka: **na tej maszynie z domyślnym sterownikiem zysk daje cache shaderów samego sterownika (na dysku), a nasz blob niczego nie dokłada**. Test to zauważa i wypisuje uwagę; twardy warunek (drugi start ≥ 1,25× szybszy od pierwszego) przechodzi w obu trybach.

W pomiarze w jednym procesie (trzy urządzenia po kolei, sterownik z cache): pierwszy 1,8 s, drugi 0,43 s, kontrola 0,36 s. To ten sam wniosek.

## Wnioski

- Blob działa: z wyłączonym cache sterownika 8 potoków kompiluje się w ~0,8 s zamiast 3–5 s, a plik ma 215 kB.
- Użytkownik z włączonym cache sterownika (na tej maszynie domyślnie) dostaje podobną poprawę **bez** naszego pliku. Własny cache potoków ma znaczenie tam, gdzie cache sterownika jest wyłączony, wyczyszczony (aktualizacja sterownika), nieobecny albo osobny na aplikację; plik jest wtedy pod kontrolą aplikacji i wersjonowany razem z nią. Na innych sterownikach (NVIDIA, Mesa, Intel) i systemach tego nie mierzono.
- Czas modułów (naga + `vkCreateShaderModule`, 4,6–5,5 s w buildzie dev dla 8 shaderów po 400 operacji, zależności bez optymalizacji) nie zależy od cache potoków; ADR-0019 przewidywał, że parsowanie WGSL może się okazać wąskim gardłem. To osobny pomiar niż M3-05 i nie robiony w release.
- Sterowniki software (lavapipe) nie trzymają cache potoków (`Empty`); test na `DeviceType::Cpu` tylko wypisuje liczby.
