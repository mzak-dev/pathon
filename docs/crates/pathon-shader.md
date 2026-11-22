# pathon-shader

Składanie shaderów WGSL z modułów (M3-01, [ADR-0021](../adr/0021-importy-shaderow-wesl.md)). Crate nie zależy od RHI, wgpu ani naga: bierze tekst, oddaje tekst, który trafia do `create_shader_module` ([ADR-0006](../adr/0006-wejscie-shaderow.md)).

- **`ShaderComposer::add_module(path, source)`** — rejestruje moduł pod ścieżką `path` (identyfikatory rozdzielone `::`, np. `colors` albo `lighting::pbr`); shader importuje go jako `package::<path>`. Dodanie modułu pod tą samą ścieżką zastępuje poprzedni, a moduły zależne od niego widzą nową treść. Moduł jest parsowany dopiero przy składaniu.
- **`ShaderComposer::compose(source, file_path)`** — składa shader od zera z aktualnych modułów i zwraca samodzielne źródło WGSL. `file_path` służy tylko do komunikatów błędów.
- **`ShaderError`** (`#[non_exhaustive]`) — `InvalidModulePath` dla złej ścieżki, `Compose` dla błędu składania (komunikat wesl bez kolorów ANSI, niezależnie od terminala i zmiennych środowiska, poprzedzony `file_path`; błędy rozwiązywania importów nie wskazują importującego pliku, więc nazwa pochodzi stąd).

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
- **Z modułów zostaje tylko to, do czego prowadzi shader główny:** nieużywane bindingi, overrides, funkcje i entry pointy modułu nie trafiają do wyniku (`strip` z `keep_main`).
- **Deklaracje modułów dostają prefiks ścieżki** (`package_colors_pack`); `_` w ścieżce i nazwie jest zamieniane na `_<liczba>` (`package_cam__1unused_tex`). Override z modułu adresuj przez `@id(n)`, który zostaje; identyfikatory `@id` są jedną przestrzenią dla shadera głównego i wszystkich modułów, duplikat odrzuca dopiero naga.
- `var<immediate>`, `enable f16;` i `enable wgpu_binding_array;` (`binding_array`) przechodzą bez zmian, także z modułów.
- Złożony WGSL nie ma komentarzy i jest przeformatowany z AST; błędy backendu wskazują linie po złożeniu (mapowanie to M3-07).
- Lista słów zarezerwowanych w wesl jest ostrzejsza niż w naga (np. funkcja `get` jest odrzucana).
- Definicje preprocesora i permutacje to M3-03.

Testy GPU (`--features gpu-tests`) składają compute z importowaną funkcją i overridem z shadera głównego, uruchamiają go i czytają bufor.

## Ograniczenia

- Składanie sprawdza tylko składnię i rozwiązywanie importów (wesl bez `eval`): błąd typów, wywołanie niezdefiniowanej funkcji czy nieznane `enable` wychodzą dopiero z naga, z pozycją w złożonym tekście. Nieużywany import nieznanego modułu jest przyjmowany po cichu.
- Błędy składni wskazują moduł (`package::lighting::pbr`), błędy rozwiązywania importów nie wskazują importującego pliku.
- `@if(FLAG)` już się parsuje, ale flag nie ma, więc zawsze wybiera `@else`; flagi to M3-03.
- Źródło z BOM jest przyjmowane (BOM jest obcinany).
- **Głębokość:** patologicznie głębokie wyrażenia lub długie łańcuchy importów mogą przepełnić stos (wesl przechodzi AST rekurencją), a nie zwrócić `Err`; limity są niższe w buildzie debug. To limit parsera wesl: źródła shaderów to kod deweloperski, a stacker ani wątek nie wchodzą w grę (ADR-0004, wasm). Szczegóły w ADR-0021.
- **Koszt:** składanie rośnie mniej więcej kwadratowo z liczbą deklaracji (kilkaset deklaracji to dziesiąte części sekundy); wynik trzeba keszować (M3-04, M3-05).
