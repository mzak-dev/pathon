use pathon_shader::ShaderComposer;

#[test]
fn error_messages_carry_no_ansi_escapes_even_when_colour_is_forced() {
    // SAFETY: this binary holds a single test, so nothing reads the environment concurrently.
    unsafe { std::env::set_var("CLICOLOR_FORCE", "1") };
    let composer = ShaderComposer::new();
    let sources = [
        "fn main( {",
        "import package::missing::gone;\n@compute @workgroup_size(1) fn main() { gone(); }",
    ];
    for source in sources {
        let message = composer
            .compose(source, "shader.wgsl")
            .expect_err("compose must fail")
            .to_string();
        assert!(!message.contains('\u{1b}'), "{message:?}");
    }
}
