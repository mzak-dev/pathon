# pathon-shader

Składanie shaderów WGSL z modułów (M3-01, [ADR-0021](../adr/0021-importy-shaderow-wesl.md)). Crate nie zależy od RHI, wgpu ani naga: bierze tekst, oddaje tekst, który trafia do `create_shader_module` ([ADR-0006](../adr/0006-wejscie-shaderow.md)).

- **`ShaderComposer::add_module(path, source)`** — rejestruje moduł pod ścieżką `path` (identyfikatory rozdzielone `::`, np. `colors` albo `lighting::pbr`); shader importuje go jako `package::<path>`. Dodanie modułu pod tą samą ścieżką zastępuje poprzedni, a moduły zależne od niego widzą nową treść. Moduł jest parsowany dopiero przy składaniu.
- **`ShaderComposer::compose(source, file_path)`** — składa shader od zera z aktualnych modułów i zwraca samodzielne źródło WGSL. `file_path` służy tylko do komunikatów błędów.
- **`ShaderError`** (`#[non_exhaustive]`) — `InvalidModulePath` dla złej ścieżki, `Compose` dla błędu składania (komunikat wesl poprzedzony `file_path`; wesl nie podaje pliku z importem, więc nazwa pochodzi stąd).

Składnia to WESL: `import package::colors::{pack, unpack};`, a `import` stoi przed `enable`.

```rust
# use pathon_shader::ShaderComposer;
# fn main() -> Result<(), pathon_shader::ShaderError> {
let mut composer = ShaderComposer::new();
composer.add_module("colors", "fn red() -> u32 { return 255u; }")?;
let wgsl = composer.compose("import package::colors::red;\nfn f() -> u32 { return red(); }", "shader.wgsl")?;
# Ok(())
# }
```

## Co zostaje, co się zmienia

- **Shader główny zachowuje nazwy** entry pointów, overrides i globali (także `pass1`, `scale0`) oraz wszystkie deklaracje, także nieużywane overrides: `ShaderStage::constants` adresuje override nazwą ([ADR-0018](../adr/0018-immediates-i-overrides.md)).
- **Deklaracje modułów dostają prefiks ścieżki** (`package_colors_pack`). Override z modułu adresuj przez `@id(n)`, który zostaje.
- `var<immediate>`, `enable f16;` i `enable wgpu_binding_array;` (`binding_array`) przechodzą bez zmian, także z modułów.
- Złożony WGSL nie ma komentarzy i jest przeformatowany z AST; błędy backendu wskazują linie po złożeniu (mapowanie to M3-07).
- Lista słów zarezerwowanych w wesl jest ostrzejsza niż w naga (np. funkcja `get` jest odrzucana).
- Definicje preprocesora i permutacje to M3-03.

Testy GPU (`--features gpu-tests`) składają compute z importowaną funkcją i overridem z shadera głównego, uruchamiają go i czytają bufor.
