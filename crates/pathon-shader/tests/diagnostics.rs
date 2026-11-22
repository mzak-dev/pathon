mod common;

use std::sync::Arc;
use std::time::Duration;

use common::TestDevice;
use pathon_api::{SerialTaskPool, Severity};
use pathon_rhi::{ComputePipelineDesc, ShaderStage};
use pathon_rhi_types::{DeviceApi, Id};
use pathon_shader::{
    BackgroundCompiler, BackgroundConfig, BuildError, Lookup, ShaderDiagnostic, ShaderError, ShaderFeatures, ShaderKey,
    ShaderLibrary, check_pipeline_constants, reflect_bind_groups,
};

const SYNTAX_ERROR: &str = include_str!("shaders/syntax_error.wgsl");
const TYPE_ERROR: &str = include_str!("shaders/type_error.wgsl");
const UNKNOWN_NAME: &str = include_str!("shaders/unknown_name.wgsl");
const BAD_BINDING: &str = include_str!("shaders/bad_binding.wgsl");
const UNKNOWN_IMPORT: &str = include_str!("shaders/unknown_import.wgsl");
const COLORS: &str = include_str!("shaders/colors.wgsl");
const BROKEN_MODULE: &str = include_str!("shaders/broken_module.wgsl");
const USES_BROKEN_MODULE: &str = include_str!("shaders/uses_broken_module.wgsl");
const WITH_OVERRIDES: &str = include_str!("shaders/with_overrides.wgsl");

fn key(root: &str, flags: &[&str]) -> ShaderKey {
    ShaderKey {
        root: root.to_owned(),
        features: flags.iter().fold(ShaderFeatures::default(), |features, flag| {
            features.with(flag).expect("flag")
        }),
    }
}

fn library_with(file: &str, source: &str) -> ShaderLibrary {
    let mut library = ShaderLibrary::new();
    library.add_root_file("shader", source, file);
    library
}

fn compile_error(library: &mut ShaderLibrary) -> ShaderDiagnostic {
    let error = library.wgsl(&key("shader", &[])).expect_err("the shader is broken");
    error.diagnostic().expect("a located diagnostic").clone()
}

fn position(diagnostic: &ShaderDiagnostic) -> (&str, u32, bool) {
    let location = diagnostic.location.as_ref().expect("a location");
    (location.file.as_str(), location.line, location.exact)
}

#[test]
fn a_syntax_error_names_the_file_the_line_and_the_column() {
    let diagnostic = compile_error(&mut library_with("shaders/syntax_error.wgsl", SYNTAX_ERROR));

    assert_eq!(diagnostic.code, "S0101");
    assert_eq!(position(&diagnostic), ("shaders/syntax_error.wgsl", 5, true));
    let location = diagnostic.location.as_ref().expect("a location");
    assert_eq!(location.line_text.trim(), "return vec4<f32>(scaled, 0.0, 1.0;");
    assert_eq!(&location.line_text[location.column as usize - 1..][..1], ";");
    assert!(
        diagnostic.hint.as_deref().is_some_and(|hint| hint.contains("`)`")),
        "{diagnostic}"
    );
}

#[test]
fn a_type_error_points_at_the_line_that_holds_it() {
    let diagnostic = compile_error(&mut library_with("shaders/type_error.wgsl", TYPE_ERROR));

    assert_eq!(diagnostic.code, "S0108");
    assert_eq!(position(&diagnostic), ("shaders/type_error.wgsl", 4, true));
    assert!(diagnostic.message.contains("u32"), "{diagnostic}");
    assert_eq!(diagnostic.location.as_ref().map(|location| location.length), Some(5));
}

#[test]
fn an_unknown_name_suggests_the_close_one() {
    let diagnostic = compile_error(&mut library_with("shaders/unknown_name.wgsl", UNKNOWN_NAME));

    assert_eq!(diagnostic.code, "S0107");
    assert_eq!(position(&diagnostic), ("shaders/unknown_name.wgsl", 5, true));
    assert_eq!(diagnostic.message, "cannot find `intensty` in this scope");
    assert_eq!(diagnostic.hint.as_deref(), Some("did you mean `intensity`?"));
}

#[test]
fn an_unknown_import_lists_what_is_registered() {
    let mut library = library_with("shaders/unknown_import.wgsl", UNKNOWN_IMPORT);
    library
        .add_module_file("colors", COLORS, "shaders/colors.wgsl")
        .expect("colors");
    let diagnostic = compile_error(&mut library);

    assert_eq!(diagnostic.code, "S0102");
    assert_eq!(position(&diagnostic), ("shaders/unknown_import.wgsl", 2, true));
    assert_eq!(diagnostic.hint.as_deref(), Some("did you mean `package::colors`?"));
}

#[test]
fn an_unknown_item_of_a_module_lists_its_declarations() {
    let mut library = library_with(
        "shaders/lit.wgsl",
        "import package::colors::gray;\n@fragment fn fs() -> @location(0) vec4<f32> { return gray(0.5); }",
    );
    library
        .add_module_file("colors", COLORS, "shaders/colors.wgsl")
        .expect("colors");
    let diagnostic = compile_error(&mut library);

    assert_eq!(diagnostic.code, "S0103");
    assert_eq!(position(&diagnostic), ("shaders/lit.wgsl", 1, true));
    assert_eq!(diagnostic.hint.as_deref(), Some("did you mean `grey`?"));
}

#[test]
fn a_resource_binding_conflict_points_at_the_second_declaration() {
    let diagnostic = compile_error(&mut library_with("shaders/bad_binding.wgsl", BAD_BINDING));

    assert_eq!(diagnostic.code, "S0109");
    assert_eq!(position(&diagnostic), ("shaders/bad_binding.wgsl", 4, true));
    assert!(
        diagnostic.hint.as_deref().is_some_and(|hint| hint.contains("@binding")),
        "{diagnostic}"
    );
}

#[test]
fn a_resource_without_a_binding_says_what_to_add() {
    let source = "var<uniform> tint: vec4<f32>;\n@fragment fn fs() -> @location(0) vec4<f32> { return tint; }";
    let diagnostic = compile_error(&mut library_with("shaders/no_binding.wgsl", source));

    assert_eq!(diagnostic.code, "S0109");
    assert_eq!(position(&diagnostic), ("shaders/no_binding.wgsl", 1, true));
    assert!(
        diagnostic.hint.as_deref().is_some_and(|hint| hint.contains("@group")),
        "{diagnostic}"
    );
}

#[test]
fn an_error_inside_a_module_points_into_the_modules_own_file() {
    let mut library = library_with("shaders/uses_broken_module.wgsl", USES_BROKEN_MODULE);
    library
        .add_module_file("broken", BROKEN_MODULE, "shaders/broken_module.wgsl")
        .expect("broken");
    let diagnostic = compile_error(&mut library);

    assert_eq!(diagnostic.code, "S0108");
    assert_eq!(position(&diagnostic), ("shaders/broken_module.wgsl", 7, true));
    let location = diagnostic.location.as_ref().expect("a location");
    assert_eq!(location.module, "package::broken");
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("shaders/uses_broken_module.wgsl")),
        "the shader that pulled the module in is named: {diagnostic}"
    );
}

#[test]
fn a_syntax_error_inside_a_module_points_into_the_modules_own_file() {
    let mut library = library_with(
        "shaders/lit.wgsl",
        "import package::cut::f;\n@compute @workgroup_size(1) fn main() { f(); }",
    );
    library
        .add_module_file("cut", "fn f() {\n    let x = ;\n}", "shaders/cut.wgsl")
        .expect("cut");
    let diagnostic = compile_error(&mut library);

    assert_eq!(diagnostic.code, "S0101");
    assert_eq!(position(&diagnostic), ("shaders/cut.wgsl", 2, true));
}

#[test]
fn names_that_composition_renamed_are_shown_as_the_developer_wrote_them() {
    let mut library = library_with(
        "shaders/lit.wgsl",
        "import package::geometry::area;\n@compute @workgroup_size(1) fn main() { _ = area(1.0); }",
    );
    library
        .add_module_file(
            "geometry",
            "fn area(side: f32) -> f32 {\n    return side * sidee;\n}",
            "shaders/geometry.wgsl",
        )
        .expect("geometry");
    let diagnostic = compile_error(&mut library);

    assert_eq!(diagnostic.code, "S0107");
    assert_eq!(position(&diagnostic), ("shaders/geometry.wgsl", 2, true));
    assert_eq!(diagnostic.hint.as_deref(), Some("did you mean `side`?"));
}

#[test]
fn a_cycle_between_modules_points_at_the_declaration() {
    let mut library = library_with(
        "shaders/lit.wgsl",
        "import package::a::fa;\n@compute @workgroup_size(1) fn main() { fa(); }",
    );
    library
        .add_module_file("a", "import package::b::fb;\nfn fa() { fb(); }", "shaders/a.wgsl")
        .expect("a");
    library
        .add_module_file("b", "import package::a::fa;\nfn fb() { fa(); }", "shaders/b.wgsl")
        .expect("b");
    let diagnostic = compile_error(&mut library);

    assert_eq!(diagnostic.code, "S0104");
    assert_eq!(position(&diagnostic), ("shaders/a.wgsl", 2, true));
    assert!(diagnostic.message.contains("a::fa"), "{diagnostic}");
}

#[test]
fn conditional_translation_mistakes_are_reported() {
    let diagnostic = compile_error(&mut library_with(
        "shaders/lit.wgsl",
        "const A: u32 = 1u;\n@if(x) @if(y) const B: u32 = 2u;\n",
    ));

    assert_eq!(diagnostic.code, "S0105");
    assert_eq!(position(&diagnostic), ("shaders/lit.wgsl", 2, true));
    assert!(
        diagnostic
            .hint
            .as_deref()
            .is_some_and(|hint| hint.contains("@if(a && b)")),
        "{diagnostic}"
    );
}

#[test]
fn a_duplicate_declaration_points_at_the_second_one() {
    let diagnostic = compile_error(&mut library_with(
        "shaders/lit.wgsl",
        "fn f() {}\nfn g() {}\nfn f() {}\n",
    ));

    assert_eq!(diagnostic.code, "S0111");
    assert_eq!(position(&diagnostic), ("shaders/lit.wgsl", 3, true));
}

#[test]
fn removed_branches_make_the_position_the_declaration_when_a_guess_would_be_wrong() {
    let source =
        "fn f() -> f32 {\n    @if(debug) { let tmp: f32 = 1.0; }\n    let tmp: u32 = 1.5;\n    return 0.0;\n}\n";
    let diagnostic = compile_error(&mut library_with("shaders/lit.wgsl", source));

    assert_eq!(diagnostic.code, "S0108");
    let location = diagnostic.location.as_ref().expect("a location");
    assert!(!location.exact, "{diagnostic}");
    assert_eq!(location.line, 1, "the declaration `f` starts on line 1: {diagnostic}");
    assert!(diagnostic.to_string().contains("where it starts"), "{diagnostic}");
}

#[test]
fn a_unique_token_is_still_found_when_a_branch_was_removed() {
    let source =
        "fn f() -> f32 {\n    @if(debug) { let tmp: f32 = 1.0; }\n    let mistake: u32 = 1.5;\n    return 0.0;\n}\n";
    let diagnostic = compile_error(&mut library_with("shaders/lit.wgsl", source));

    assert_eq!(position(&diagnostic), ("shaders/lit.wgsl", 3, true));
}

#[test]
fn a_flag_nobody_reads_is_a_warning_with_a_suggestion() {
    let mut library = library_with(
        "shaders/lit.wgsl",
        "@if(shadows) const LEVEL: u32 = 2u;\n@else const LEVEL: u32 = 1u;\n@compute @workgroup_size(1) fn main() { _ = LEVEL; }",
    );

    library.wgsl(&key("shader", &["shadw"])).expect("composes");
    let warnings = library.take_warnings();

    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, "S0106");
    assert_eq!(warnings[0].severity, Severity::Warning);
    assert_eq!(warnings[0].hint.as_deref(), Some("did you mean `shadows`?"));
    library.wgsl(&key("shader", &["shadows"])).expect("composes");
    assert!(library.take_warnings().is_empty(), "a flag that is read does not warn");
}

#[test]
fn a_constant_naming_no_override_lists_the_shaders_overrides() {
    let mut library = library_with("shaders/with_overrides.wgsl", WITH_OVERRIDES);
    let wgsl = library.wgsl(&key("shader", &[])).expect("composes");
    let stage = |name: &'static str| ShaderStage {
        module: pathon_rhi::ShaderModuleId::from_raw(pathon_rhi_types::RawHandle::new(0, 0)),
        entry_point: "main",
        constants: vec![(name, 2.0.into())],
    };

    check_pipeline_constants(&wgsl, &stage("intensity")).expect("a declared override");
    check_pipeline_constants(&wgsl, &stage("7")).expect("an override is also addressed by its @id");
    let error = check_pipeline_constants(&wgsl, &stage("intensty")).expect_err("no such override");

    let diagnostic = error.diagnostic().expect("a diagnostic");
    assert_eq!(diagnostic.code, "S0110");
    assert_eq!(diagnostic.hint.as_deref(), Some("did you mean `intensity`?"));
    let unrelated = check_pipeline_constants(&wgsl, &stage("zzz")).expect_err("no such override");
    let hint = unrelated
        .diagnostic()
        .and_then(|diagnostic| diagnostic.hint.clone())
        .expect("a hint");
    assert!(hint.contains("`intensity`") && hint.contains("`samples`"), "{hint}");
}

#[test]
fn the_background_compiler_reports_a_located_diagnostic_for_a_broken_shader() {
    let device = Arc::new(TestDevice::new(Duration::ZERO));
    let handle: Arc<dyn DeviceApi> = Arc::<TestDevice>::clone(&device);
    let mut compiler = BackgroundCompiler::new(handle, Arc::new(SerialTaskPool), BackgroundConfig::new());
    compiler.add_root_file("broken", TYPE_ERROR, "shaders/type_error.wgsl");

    let broken = key("broken", &[]);
    assert_eq!(compiler.shader_module(&broken), Lookup::Pending);
    assert!(compiler.finish_all(Duration::from_secs(5)));
    assert_eq!(compiler.shader_module(&broken), Lookup::Failed);

    let failures = compiler.take_failures();
    assert_eq!(failures.len(), 1);
    assert!(matches!(
        failures[0].error,
        BuildError::Shader(ShaderError::InvalidWgsl { .. })
    ));
    assert_eq!(failures[0].code(), "S0108");
    let diagnostic = failures[0].diagnostic();
    assert_eq!(diagnostic.code, "S0108");
    assert!(
        diagnostic.message.contains("shaders/type_error.wgsl:4:9"),
        "{}",
        diagnostic.message
    );
    assert!(
        diagnostic.message.contains("var count: u32 = 1.5;"),
        "{}",
        diagnostic.message
    );
    let text = diagnostic.to_string();
    assert!(text.starts_with("error[S0108]"), "{text}");
    assert!(text.contains("docs/diag/S0108.md"), "{text}");
}

#[test]
fn the_background_compiler_checks_pipeline_constants_against_the_module() {
    let device = Arc::new(TestDevice::new(Duration::ZERO));
    let handle: Arc<dyn DeviceApi> = Arc::<TestDevice>::clone(&device);
    let mut compiler = BackgroundCompiler::new(handle, Arc::new(SerialTaskPool), BackgroundConfig::new());
    compiler.add_root_file("overrides", WITH_OVERRIDES, "shaders/with_overrides.wgsl");
    let overrides = key("overrides", &[]);
    compiler.shader_module(&overrides);
    assert!(compiler.finish_all(Duration::from_secs(5)));
    let module = compiler.shader_module(&overrides).ready().expect("module");

    let desc = |constant: &'static str| ComputePipelineDesc {
        label: "overrides",
        bind_group_layouts: vec![],
        shader: ShaderStage {
            module,
            entry_point: "main",
            constants: vec![(constant, 3.0.into())],
        },
        immediate_size: 0,
    };
    let good = desc("intensity");
    let typo = desc("intensty");

    compiler.compute_pipeline(&good);
    compiler.compute_pipeline(&typo);
    assert!(compiler.finish_all(Duration::from_secs(5)));

    assert!(compiler.compute_pipeline(&good).ready().is_some());
    assert_eq!(compiler.compute_pipeline(&typo), Lookup::Failed);
    let failures = compiler.take_failures();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].code(), "S0110");
    assert!(
        failures[0]
            .diagnostic()
            .hint
            .expect("a hint")
            .contains("did you mean `intensity`?")
    );
}

#[test]
fn reflection_reports_where_in_the_given_wgsl_the_problem_is() {
    let error = reflect_bind_groups("fn main() {\n    let x: u32 = 1.5;\n}\n", &["main"]).expect_err("invalid");
    let diagnostic = error.diagnostic().expect("a located diagnostic");

    assert_eq!(diagnostic.code, "S0108");
    assert_eq!(diagnostic.location.as_ref().map(|location| location.line), Some(2));
}

#[test]
fn the_rendered_diagnostic_reads_like_rustc() {
    let diagnostic = compile_error(&mut library_with("shaders/unknown_name.wgsl", UNKNOWN_NAME));
    let rendered = diagnostic.to_string();

    let expected = "error[S0107]: cannot find `intensty` in this scope
 --> shaders/unknown_name.wgsl:5:15
  |
5 |     let lit = intensty * 2.0;
  |               ^^^^^^^^
  = hint: did you mean `intensity`?
  = docs: docs/diag/S0107.md";
    assert_eq!(rendered, expected);
}

#[test]
fn the_pathon_diag_form_carries_file_line_snippet_and_hint() {
    let diagnostic = compile_error(&mut library_with("shaders/unknown_name.wgsl", UNKNOWN_NAME)).to_diagnostic();

    assert_eq!(diagnostic.code, "S0107");
    assert_eq!(diagnostic.severity, Severity::Error);
    assert!(
        diagnostic
            .message
            .starts_with("shaders/unknown_name.wgsl:5:15: cannot find"),
        "{}",
        diagnostic.message
    );
    assert!(diagnostic.message.contains("^^^^^^^^"), "{}", diagnostic.message);
    assert_eq!(diagnostic.hint.as_deref(), Some("did you mean `intensity`?"));
}
