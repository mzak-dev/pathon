mod common;

use std::fs;
use std::path::Path;

use common::{Rig, TempDir, codes, key, write};
use pathon_hot::{HotConfig, HotError, HotReloader, SourceLimits};

const SMALL: &str = "@compute @workgroup_size(1)\nfn main() { let v = 1.0; }\n";

fn pump_for_code(rig: &mut Rig, code: &'static str) -> Vec<pathon_hot::HotReport> {
    rig.pump_until(code, |reports| codes(reports).contains(&code))
}

#[test]
fn a_file_over_the_size_limit_is_refused_and_the_old_one_stays() {
    let mut rig = Rig::with_config(&[("big", SMALL)], &[], |config| {
        config.with_limits(SourceLimits::new().with_max_file_bytes(512))
    });
    let module = rig.build(&key("big"));

    write(
        &rig.dir.join("shaders/big.wgsl"),
        &format!("{SMALL}// {}\n", "x".repeat(2000)),
    );
    let reports = pump_for_code(&mut rig, "K0002");

    assert!(reports.iter().all(|report| !report.changed()));
    assert_eq!(rig.compiler.shader_module(&key("big")).ready(), Some(module));
}

#[test]
fn a_file_that_is_not_utf8_is_refused() {
    let mut rig = Rig::new(&[("text", SMALL)], &[]);
    rig.build(&key("text"));

    fs::write(rig.dir.join("shaders/text.wgsl"), [0x66_u8, 0x6e, 0xff, 0xfe, 0x00]).expect("write");
    let reports = pump_for_code(&mut rig, "K0003");

    assert!(reports.iter().all(|report| !report.changed()));
    assert!(rig.compiler.wgsl(&key("text")).expect("still there").contains("1.0"));
}

#[test]
fn a_module_file_that_is_no_identifier_is_refused() {
    let mut rig = Rig::new(&[], &[]);
    write(&rig.dir.join("modules/not-an-identifier.wgsl"), "fn f() {}");
    pump_for_code(&mut rig, "K0004");
}

#[test]
fn files_with_other_extensions_are_ignored() {
    let mut rig = Rig::new(&[("real", SMALL)], &[]);
    write(&rig.dir.join("shaders/notes.txt"), "not a shader");
    write(&rig.dir.join("shaders/real.wgsl.tmp"), "staging");
    let reports: Vec<_> = (0..20)
        .map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            rig.pump()
        })
        .collect();
    assert!(
        reports
            .iter()
            .all(|report| !report.changed() && report.diagnostics.is_empty())
    );
}

// The process must survive sources that would overflow wesl's stack (PAT-122); each reaches the reloader as an edit of
// a shader that works.
fn refuses_edit_to(source: &str) {
    let mut rig = Rig::new(&[("deep", SMALL)], &[]);
    let module = rig.build(&key("deep"));

    write(&rig.dir.join("shaders/deep.wgsl"), source);
    let reports = pump_for_code(&mut rig, "K0005");

    assert!(reports.iter().all(|report| !report.changed()));
    assert_eq!(
        rig.compiler.shader_module(&key("deep")).ready(),
        Some(module),
        "the working shader is untouched"
    );
}

#[test]
fn deeply_nested_parentheses_are_refused_instead_of_crashing_the_process() {
    let depth = 200_000;
    refuses_edit_to(&format!(
        "@compute @workgroup_size(1)\nfn main() {{ let v = {}1.0{}; }}",
        "(".repeat(depth),
        ")".repeat(depth)
    ));
}

#[test]
fn deeply_nested_blocks_are_refused_instead_of_crashing_the_process() {
    let depth = 100_000;
    refuses_edit_to(&format!(
        "@compute @workgroup_size(1)\nfn main() {}{}",
        "{".repeat(depth),
        "}".repeat(depth)
    ));
}

#[test]
fn a_chain_of_a_hundred_thousand_operators_is_refused_instead_of_crashing_the_process() {
    refuses_edit_to(&format!(
        "@compute @workgroup_size(1)\nfn main() {{ let v = 1.0{}; }}",
        " + 1.0".repeat(100_000)
    ));
}

#[test]
fn a_chain_of_imports_deeper_than_the_limit_is_refused_instead_of_crashing_the_process() {
    let mut rig = Rig::new(&[("chain", SMALL)], &[]);
    let module = rig.build(&key("chain"));

    let depth = 400;
    for index in 0..depth {
        let next = index + 1;
        let body = if next < depth {
            format!("import package::m{next}::f{next};\nfn f{index}() -> f32 {{ return f{next}(); }}")
        } else {
            format!("fn f{index}() -> f32 {{ return 1.0; }}")
        };
        write(&rig.dir.join(&format!("modules/m{index}.wgsl")), &body);
    }
    write(
        &rig.dir.join("shaders/chain.wgsl"),
        "import package::m0::f0;\n@compute @workgroup_size(1) fn main() { let v = f0(); }",
    );
    let reports = pump_for_code(&mut rig, "K0005");

    assert!(
        reports.iter().all(|report| !report.changed()),
        "no module of the chain was taken"
    );
    assert_eq!(rig.compiler.shader_module(&key("chain")).ready(), Some(module));
}

#[test]
fn a_chain_of_imports_present_at_start_is_refused_too() {
    let dir = TempDir::new("deep-start");
    let depth = 200;
    for index in 0..depth {
        let body = if index + 1 < depth {
            format!(
                "import package::m{}::f{};\nfn f{index}() -> f32 {{ return f{}(); }}",
                index + 1,
                index + 1,
                index + 1
            )
        } else {
            format!("fn f{index}() -> f32 {{ return 1.0; }}")
        };
        write(&dir.join(&format!("modules/m{index}.wgsl")), &body);
    }
    fs::create_dir_all(dir.join("shaders")).expect("dir");
    let config = HotConfig::new()
        .with_shader_dir(dir.join("shaders"))
        .with_module_dir(dir.join("modules"));
    let mut reloader = HotReloader::new(&config).expect("starts");
    let mut compiler = pathon_shader::BackgroundCompiler::new(
        std::sync::Arc::new(pathon_rhi::Device::null()),
        std::sync::Arc::new(pathon_api::SerialTaskPool),
        pathon_shader::BackgroundConfig::new(),
    );

    let report = reloader.pump(&mut compiler);

    assert!(codes(std::slice::from_ref(&report)).contains(&"K0005"));
    assert!(
        report.reloaded.len() < depth,
        "{} of {depth} modules were taken",
        report.reloaded.len()
    );
}

#[test]
fn starting_without_a_directory_or_on_a_missing_one_is_an_error() {
    assert!(matches!(
        HotReloader::new(&HotConfig::new()),
        Err(HotError::NoDirectories)
    ));
    let missing = TempDir::new("missing").join("nope");
    let config = HotConfig::new().with_shader_dir(missing);
    assert!(matches!(
        HotReloader::new(&config),
        Err(HotError::DirectoryUnavailable { .. })
    ));
}

fn try_link_file(target: &Path, link: &Path) -> bool {
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_file(target, link);
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(target, link);
    result.is_ok()
}

fn try_link_dir(target: &Path, link: &Path) -> bool {
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_dir(target, link);
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(target, link);
    result.is_ok()
}

#[test]
fn a_link_that_leaves_the_watched_directory_is_not_followed() {
    let outside = TempDir::new("outside");
    write(&outside.join("secret.wgsl"), SMALL);
    write(&outside.join("dir/inside.wgsl"), SMALL);
    let mut rig = Rig::new(&[("real", SMALL)], &[]);
    if !try_link_file(&outside.join("secret.wgsl"), &rig.dir.join("shaders/stolen.wgsl")) {
        eprintln!("symbolic links are not permitted here; the link test is skipped");
        return;
    }
    let linked_dir = try_link_dir(&outside.join("dir"), &rig.dir.join("shaders/linked"));

    let reports = pump_for_code(&mut rig, "K0001");
    let more: Vec<_> = (0..10).map(|_| rig.pump()).collect();

    assert!(
        reports.iter().chain(&more).all(|report| !report.changed()),
        "nothing outside was registered"
    );
    assert_eq!(
        rig.compiler.shader_module(&key("stolen")),
        pathon_shader::Lookup::Failed
    );
    if linked_dir {
        assert_eq!(
            rig.compiler.shader_module(&key("linked/inside")),
            pathon_shader::Lookup::Failed
        );
    }
}

#[test]
fn a_link_to_a_file_inside_the_directory_is_read() {
    let mut rig = Rig::new(&[("target", SMALL)], &[]);
    if !try_link_file(
        &rig.dir.join("shaders/target.wgsl"),
        &rig.dir.join("shaders/alias.wgsl"),
    ) {
        eprintln!("symbolic links are not permitted here; the link test is skipped");
        return;
    }
    let reports = rig.pump_until_reloaded("the alias");
    assert!(
        reports
            .iter()
            .any(|report| report.reloaded.iter().any(|label| label.ends_with("alias.wgsl")))
    );
}
