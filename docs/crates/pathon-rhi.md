# pathon-rhi

Fasada RHI (M0-09): re-eksport typów z `pathon-rhi-types` i `enum Device` z wariantem na każdy backend. Rdzeń mówi tylko do tego crate'u.

- `Device::build(&request)` — natywnie, blokująco; `Device::build_async(&request).await` — także na wasm. `DeviceRequest::backend = Some(Backend::Null)` daje backend walidujący bez GPU.
- `Device` implementuje `DeviceApi` przez delegację do wariantu, więc rdzeń nie używa `dyn` ani generyków.
- `as_wgpu()` / `as_null()` zwracają konkretny backend — np. adapter hosta sięga przez to po `interop`.

Test zgodności (`tests/conformance.rs`, M1-06) przechodzi te same kroki na każdym backendzie: kopie bufor ↔ tekstura, compute, rasteryzacja z buforem indeksów i teksturą z samplerem, timestampy passów. Na `null` sprawdza plan stanów, na wgpu także wynik w pikselach i bajtach:

```sh
cargo test -p pathon-rhi                      # null
cargo test -p pathon-rhi --features gpu-tests # null + wgpu na prawdziwym GPU
```
