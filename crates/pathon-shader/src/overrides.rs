use naga::front::wgsl;
use pathon_rhi_types::ShaderStage;

use crate::ShaderError;
use crate::diagnostic::{ShaderDiagnostic, UNKNOWN_OVERRIDE};
use crate::locate::unmangle_text;
use crate::text::{closest, list_of};

/// Checks the names in `stage.constants` against the overrides the composed `wgsl` declares (ADR-0018): a constant
/// that names no override is otherwise only found by the driver, as a pipeline error without the list of names.
///
/// An override is addressed by its name, or by its `@id` number when it has one. Text naga cannot parse passes, so
/// the syntax error is reported where it belongs: when the module is created.
pub fn check_pipeline_constants(wgsl: &str, stage: &ShaderStage) -> Result<(), ShaderError> {
    if stage.constants.is_empty() {
        return Ok(());
    }
    let Ok(module) = wgsl::parse_str(wgsl) else {
        return Ok(());
    };
    let declared: Vec<(String, String)> = module
        .overrides
        .iter()
        .map(|(_, declaration)| {
            let name = unmangle_text(declaration.name.as_deref().unwrap_or("_"));
            let key = declaration.id.map_or_else(|| name.clone(), |id| id.to_string());
            (key, name)
        })
        .collect();
    for (constant, _) in &stage.constants {
        if declared.iter().any(|(key, _)| key == constant) {
            continue;
        }
        let names: Vec<&str> = declared.iter().map(|(_, name)| name.as_str()).collect();
        let hint = match closest(constant, declared.iter().map(|(key, _)| key.as_str())) {
            Some(found) => format!("did you mean `{found}`?"),
            None if declared.is_empty() => "the shader declares no `override`".to_owned(),
            None => format!("the shader declares: {}", list_of(&names, 8)),
        };
        let diagnostic = ShaderDiagnostic::error(
            UNKNOWN_OVERRIDE,
            format!(
                "constant `{constant}` of entry point `{}` names no override of the shader",
                stage.entry_point
            ),
        )
        .with_hint(hint);
        return Err(ShaderError::Diagnostic(Box::new(diagnostic)));
    }
    Ok(())
}
