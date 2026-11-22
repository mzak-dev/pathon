# Zrzut z Tracy: nazwane passy (M2-12)

Dowód do „Gotowe gdy” M2-12: zrzut `pathon.tracy` z przykładu `profile` z `--features tracy`, z nazwami passów z grafu zarówno w strefach CPU, jak i w strefach GPU.

- **Narzędzia:** Tracy v0.14.1 (oficjalne wydanie z github.com/wolfpld/tracy; wersja klienta w `tracy-client` 0.19), użyte: `tracy-capture.exe` i `tracy-csvexport.exe`.
- **Maszyna:** AMD Radeon RX 9070 XT (Vulkan), Windows 11, profil `dev`, 22.11.2026.
- **Zrzut:** `cargo run -p pathon-examples --bin profile --features tracy -- 900`, równocześnie `tracy-capture -o pathon.tracy -f -a 127.0.0.1`. Wynik: 902 klatki, 1800 stref CPU, 36,6 KB.
- `zones.csv` — `tracy-csvexport pathon.tracy`: strefy CPU `clear output` i `unlit mesh`, po 900 wywołań (nazwy z `PassDesc::name`).
- `gpu-summary.csv` — podsumowanie `tracy-csvexport -g pathon.tracy`: te same dwie nazwy w kontekście GPU `pathon GPU`, po 900 stref, czasy GPU z timestampów (mikrosekundy; `unlit mesh` średnio 6,0 µs).
