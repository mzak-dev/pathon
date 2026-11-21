# pathon-shader

Składanie shaderów WGSL z modułów (M3-01, [ADR-0021](../adr/0021-importy-shaderow-naga-oil.md)). Crate nie zależy od RHI ani od wgpu: bierze tekst, oddaje tekst, który trafia do `create_shader_module` ([ADR-0006](../adr/0006-wejscie-shaderow.md)).

- **`ShaderComposer::add_module(source, file_path)`** — rejestruje moduł z `#define_import_path pathon::nazwa`. Moduł jest parsowany leniwie (przy pierwszym użyciu w składaniu), ale to, czego używa z importów, musi być dodane wcześniej, więc cykl importów kończy się błędem (nie znaleziono modułu), a nie pętlą.
- **`ShaderComposer::compose(source, file_path)`** — rozwiązuje `#import pathon::nazwa::{funkcja, STALA}` i zwraca samodzielne źródło WGSL, zapisane z `naga::Module` po walidacji.
- **Ten sam path dodany drugi raz zastępuje moduł** (zachowanie `naga_oil`). Zastąpienie widzą kolejne składania; wymianę modułu, od którego zależą już dodane moduły, rób przed ich dodaniem.
- **`ShaderError`** (`#[non_exhaustive]`) — `Compose` niesie komunikat `naga_oil` z nazwą pliku i linią, `WriteWgsl` błąd walidacji lub zapisu WGSL.

Przykład:

```rust
# use pathon_shader::ShaderComposer;
# fn main() -> Result<(), pathon_shader::ShaderError> {
let mut composer = ShaderComposer::new();
composer.add_module(
    "#define_import_path pathon::colors\nfn red() -> u32 { return 255u; }",
    "colors.wgsl",
)?;
let wgsl = composer.compose("#import pathon::colors::{red}\nfn f() -> u32 { return red(); }", "shader.wgsl")?;
# Ok(())
# }
```

Zapisany WGSL różni się od wejścia: nazwy z importów dostają prefiks `naga_oil`, komentarze znikają, a błędy backendu wskazują linie po złożeniu (mapowanie to M3-07). Definicje preprocesora (`#ifdef`) czekają na M3-03.

Testy GPU (`--features gpu-tests`) składają compute z importowaną funkcją i stałą, uruchamiają go i czytają bufor.
