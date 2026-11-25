mod common;

use std::fs;
use std::time::Duration;

use common::{Rig, codes, key, write, write_atomically};
use pathon_hot::HotReport;
use pathon_rhi::{ComputePipelineDesc, Resource, ShaderStage};
use pathon_shader::Lookup;

const FILL: &str = "
import package::colors::tint;

@compute @workgroup_size(1)
fn main() { let value = tint(1.0); }
";

const COLORS: &str = "fn tint(x: f32) -> f32 { return x * 2.0; }";

fn fill_with(factor: &str) -> String {
    format!("@compute @workgroup_size(1)\nfn main() {{ let value = 1.0 * {factor}; }}\n")
}

fn compute_desc(module: pathon_rhi::ShaderModuleId) -> ComputePipelineDesc {
    ComputePipelineDesc {
        label: "hot",
        bind_group_layouts: vec![],
        shader: ShaderStage {
            module,
            entry_point: "main",
            constants: vec![],
        },
        immediate_size: 0,
    }
}

fn wgsl_of(rig: &Rig, root: &str) -> String {
    rig.compiler.wgsl(&key(root)).expect("built").to_string()
}

#[test]
fn files_on_disk_are_registered_before_the_first_frame() {
    let mut rig = Rig::new(&[("fill", FILL)], &[("colors", COLORS)]);
    let module = rig.build(&key("fill"));
    assert!(wgsl_of(&rig, "fill").contains("* 2.0"));
    assert_eq!(rig.compiler.shader_module(&key("fill")), Lookup::Ready(module));
}

#[test]
fn a_plain_write_rebuilds_the_shader_without_a_restart() {
    let mut rig = Rig::new(&[("plain", &fill_with("2.0"))], &[]);
    let before = rig.build(&key("plain"));

    write(&rig.dir.join("shaders/plain.wgsl"), &fill_with("3.0"));
    let reports = rig.pump_until_reloaded("the plain write");

    assert!(
        reports
            .iter()
            .any(|report| report.reloaded.iter().any(|label| label.ends_with("plain.wgsl")))
    );
    let after = rig.build(&key("plain"));
    assert_ne!(before, after);
    assert!(wgsl_of(&rig, "plain").contains("* 3.0"));
}

#[test]
fn an_atomic_replace_rebuilds_the_shader_without_a_restart() {
    let mut rig = Rig::new(&[("atomic", &fill_with("2.0"))], &[]);
    rig.build(&key("atomic"));

    write_atomically(&rig.dir.join("shaders/atomic.wgsl"), &fill_with("5.0"));
    rig.pump_until_reloaded("the atomic replace");

    rig.build(&key("atomic"));
    assert!(wgsl_of(&rig, "atomic").contains("* 5.0"));
}

#[test]
fn a_changed_module_rebuilds_the_roots_that_import_it() {
    let mut rig = Rig::new(&[("fill", FILL)], &[("colors", COLORS)]);
    rig.build(&key("fill"));

    write(
        &rig.dir.join("modules/colors.wgsl"),
        "fn tint(x: f32) -> f32 { return x * 7.0; }",
    );
    rig.pump_until_reloaded("the module");

    rig.build(&key("fill"));
    assert!(wgsl_of(&rig, "fill").contains("* 7.0"));
}

#[test]
fn a_shader_that_does_not_compile_leaves_the_previous_version_in_use() {
    let mut rig = Rig::new(&[("keep", &fill_with("2.0"))], &[]);
    let good = rig.build(&key("keep"));

    write(
        &rig.dir.join("shaders/keep.wgsl"),
        "@compute @workgroup_size(1)\nfn main() {\n    let value = missing_name * 2.0;\n}\n",
    );
    let reports = rig.pump_until("the rejection", |reports| codes(reports).contains(&"K0006"));

    let all = codes(&reports);
    assert!(all.contains(&"S0107"), "the located error is reported: {all:?}");
    let located = reports
        .iter()
        .flat_map(|report| &report.diagnostics)
        .find(|diagnostic| diagnostic.code == "S0107")
        .expect("located error");
    assert!(
        located.message.contains("keep.wgsl:3:"),
        "file and line: {}",
        located.message
    );
    assert!(reports.iter().all(|report| !report.changed()), "nothing was applied");
    assert!(reports.iter().all(|report| report.retired.is_empty()));
    assert_eq!(
        rig.compiler.shader_module(&key("keep")),
        Lookup::Ready(good),
        "the working module is still there"
    );
    assert!(wgsl_of(&rig, "keep").contains("* 2.0"));

    write(&rig.dir.join("shaders/keep.wgsl"), &fill_with("4.0"));
    rig.pump_until_reloaded("the fix");
    let fixed = rig.build(&key("keep"));
    assert_ne!(good, fixed);
    assert!(wgsl_of(&rig, "keep").contains("* 4.0"));
}

#[test]
fn the_same_mistake_saved_again_is_reported_once() {
    let mut rig = Rig::new(&[("once", &fill_with("2.0"))], &[]);
    rig.build(&key("once"));
    let broken = "@compute @workgroup_size(1)\nfn main() { let a = nope; }\n";

    write(&rig.dir.join("shaders/once.wgsl"), broken);
    let first = rig.pump_until("the first report", |reports| codes(reports).contains(&"K0006"));
    write(&rig.dir.join("shaders/once.wgsl"), &format!("{broken}// saved again\n"));
    std::thread::sleep(Duration::from_millis(200));
    let second: Vec<HotReport> = (0..20).map(|_| rig.pump()).collect();

    assert_eq!(codes(&first).iter().filter(|code| **code == "K0006").count(), 1);
    assert!(!codes(&second).contains(&"K0006"), "{:?}", codes(&second));
}

#[test]
fn one_broken_file_does_not_hold_back_another() {
    let mut rig = Rig::new(&[("good", &fill_with("2.0")), ("bad", &fill_with("2.0"))], &[]);
    rig.build(&key("good"));
    rig.build(&key("bad"));

    write(
        &rig.dir.join("shaders/bad.wgsl"),
        "@compute @workgroup_size(1) fn main() { let a = nope; }",
    );
    write(&rig.dir.join("shaders/good.wgsl"), &fill_with("9.0"));
    let reports = rig.pump_until("both files handled", |reports| {
        reports.iter().any(HotReport::changed) && codes(reports).contains(&"K0006")
    });

    let reloaded: Vec<&String> = reports.iter().flat_map(|report| &report.reloaded).collect();
    assert_eq!(reloaded.len(), 1);
    assert!(reloaded[0].ends_with("good.wgsl"));
    rig.build(&key("good"));
    assert!(wgsl_of(&rig, "good").contains("* 9.0"));
    assert!(wgsl_of(&rig, "bad").contains("* 2.0"));
}

#[test]
fn a_module_and_the_root_that_needs_it_arrive_together() {
    let mut rig = Rig::new(&[("pair", &fill_with("2.0"))], &[]);
    rig.build(&key("pair"));

    write(
        &rig.dir.join("shaders/pair.wgsl"),
        "import package::fresh::scale;\n@compute @workgroup_size(1) fn main() { let v = scale(1.0); }\n",
    );
    write(
        &rig.dir.join("modules/fresh.wgsl"),
        "fn scale(x: f32) -> f32 { return x * 11.0; }",
    );
    let reports = rig.pump_until_reloaded("both files");

    assert!(!codes(&reports).contains(&"K0006"));
    rig.build(&key("pair"));
    assert!(wgsl_of(&rig, "pair").contains("* 11.0"));
}

#[test]
fn a_deleted_shader_is_dropped() {
    let mut rig = Rig::new(&[("gone", &fill_with("2.0"))], &[]);
    rig.build(&key("gone"));

    fs::remove_file(rig.dir.join("shaders/gone.wgsl")).expect("delete");
    let reports = rig.pump_until("the deletion", |reports| reports.iter().any(HotReport::changed));

    assert!(
        reports
            .iter()
            .any(|report| report.removed.iter().any(|label| label.ends_with("gone.wgsl")))
    );
    assert_eq!(rig.compiler.shader_module(&key("gone")), Lookup::Failed);
}

#[test]
fn replaced_modules_and_pipelines_are_handed_back_only_once_the_replacements_are_built() {
    let mut rig = Rig::new(&[("pipes", &fill_with("2.0"))], &[]);
    let old_module = rig.build(&key("pipes"));
    let old_desc = compute_desc(old_module);
    let old_pipeline = loop {
        rig.compiler.pump();
        if let Lookup::Ready(pipeline) = rig.compiler.compute_pipeline(&old_desc) {
            break pipeline;
        }
    };

    write(&rig.dir.join("shaders/pipes.wgsl"), &fill_with("3.0"));
    let reports = rig.pump_until_reloaded("the change");
    assert!(
        reports.iter().all(|report| report.retired.is_empty()),
        "nothing is retired while the old pipeline may be drawing"
    );
    assert_eq!(
        rig.compiler.compute_pipeline(&old_desc),
        Lookup::Failed,
        "the stale pipeline is no longer offered"
    );

    let new_module = rig.build(&key("pipes"));
    let new_desc = compute_desc(new_module);
    let new_pipeline = loop {
        rig.compiler.pump();
        if let Lookup::Ready(pipeline) = rig.compiler.compute_pipeline(&new_desc) {
            break pipeline;
        }
    };
    let retired: Vec<Resource> = rig
        .pump_until("the old resources to be handed back", |reports| {
            reports.iter().any(|report| !report.retired.is_empty())
        })
        .into_iter()
        .flat_map(|report| report.retired)
        .collect();

    assert_eq!(retired.len(), 2, "{retired:?}");
    assert!(retired.contains(&Resource::from(old_module)));
    assert!(retired.contains(&Resource::from(old_pipeline)));
    assert!(!retired.contains(&Resource::from(new_pipeline)));
    assert_eq!(rig.compiler.compute_pipeline(&new_desc), Lookup::Ready(new_pipeline));
}

#[test]
fn a_build_that_fails_in_the_background_keeps_the_old_resources_alive() {
    // Passes the check on the composed text (naga with every capability) but not the pipeline's override check.
    let mut rig = Rig::new(
        &[(
            "late",
            "override gain: f32 = 1.0;\n@compute @workgroup_size(1) fn main() { let v = gain; }",
        )],
        &[],
    );
    let old_module = rig.build(&key("late"));

    write(
        &rig.dir.join("shaders/late.wgsl"),
        "override other: f32 = 1.0;\n@compute @workgroup_size(1) fn main() { let v = other; }",
    );
    rig.pump_until_reloaded("the change");
    let new_module = rig.build(&key("late"));
    let mut desc = compute_desc(new_module);
    desc.shader.constants = vec![("gain", 2.0_f64.into())];
    assert_eq!(rig.compiler.compute_pipeline(&desc), Lookup::Pending);

    let reports = rig.pump_until("the failure report", |reports| codes(reports).contains(&"S0110"));
    let later: Vec<HotReport> = (0..10).map(|_| rig.pump()).collect();

    assert!(reports.iter().chain(&later).all(|report| report.retired.is_empty()));
    assert!(
        rig.reloader.take_retired().contains(&Resource::from(old_module)),
        "the old module is still kept"
    );
}
