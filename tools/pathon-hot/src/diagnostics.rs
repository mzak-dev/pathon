use pathon_diag::Diagnostic;

pub(crate) fn outside_root(file: &str, detail: &str) -> Diagnostic {
    Diagnostic::warning("K0001", format!("`{file}` is skipped: {detail}"))
        .with_hint("hot reload reads only files under the watched directories and follows no symbolic links out of them")
}

pub(crate) fn too_large(file: &str, size: u64, limit: u64) -> Diagnostic {
    Diagnostic::warning("K0002", format!("`{file}` is skipped: {size} bytes, over the limit of {limit}"))
        .with_hint("raise `SourceLimits::max_file_bytes` or split the shader into modules")
}

pub(crate) fn unreadable(file: &str, detail: &str) -> Diagnostic {
    Diagnostic::warning("K0003", format!("`{file}` is skipped: {detail}"))
        .with_hint("shader files must be UTF-8 text; the previous version stays in use")
}

pub(crate) fn unusable_name(file: &str, detail: &str) -> Diagnostic {
    Diagnostic::warning("K0004", format!("`{file}` is skipped: {detail}"))
        .with_hint("name module files with identifiers (letters, digits, `_`) that are not WGSL keywords")
}

pub(crate) fn too_deep(file: &str, line: usize, detail: &str) -> Diagnostic {
    Diagnostic::warning("K0005", format!("`{file}` line {line}: {detail}")).with_hint(
        "wesl walks the syntax tree recursively and would overflow the stack; simplify the source or raise `SourceLimits`",
    )
}

pub(crate) fn too_deep_imports(chain: &str, depth: usize, limit: usize) -> Diagnostic {
    Diagnostic::warning(
        "K0005",
        format!("modules import each other {depth} levels deep (limit {limit}), through {chain}"),
    )
    .with_hint("flatten the module imports or raise `SourceLimits::max_import_depth`")
}

pub(crate) fn reload_rejected(file: &str) -> Diagnostic {
    Diagnostic::error(
        "K0006",
        format!("the change to `{file}` is not applied: the previous version stays in use"),
    )
    .with_hint("fix the error above and save again; nothing stops drawing meanwhile")
}

pub(crate) fn watcher_problem(detail: &str) -> Diagnostic {
    Diagnostic::warning("K0007", detail.to_owned())
        .with_hint("the directories are read again after every such event, so a change is not lost")
}
