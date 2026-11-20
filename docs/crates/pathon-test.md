# pathon-test

Paczki testowe, które napierają na granicę rdzeń / API (M2-14), oraz paczka pionowego wycinka (M2-18). Wszystkie są zwykłymi paczkami: zależą tylko od `pathon-api`.

- **`GeometryPack`** — zajmuje rolę Geometry i wypełnia `Hdr`; `Roughness` produkuje tylko na żądanie. Dwie instancje sprawdzają podmianę roli (druga dostaje F0102).
- **`UpscalerPack`** — rola `TemporalResolve`: w `prepare_view` zmniejsza `render_size` i ustawia jitter, w `record` kopiuje `Hdr` (rozmiar renderu) do `Output` (rozmiar wyjścia). Zapisuje rozmiar, który zobaczył, do testu.
- **`OnDemandConsumer`** — prosi o slot na żądanie (`Roughness`) i go czyta. Zastępuje „paczkę z hookiem materiału” do czasu systemu materiałów (M4-03), który da prawdziwe hooki WGSL.
- **`VendorDummy`** — atrapa paczki vendorowej: `ExternalCommand` z zadeklarowanym dostępem do `Hdr`.
- **`UnlitMeshPack`** — pionowy wycinek: kolorowy sześcian z zaszytym shaderem WGSL, rysowany wprost do `Output` z własną głębią; pipeline na format celu tworzony leniwie.
