// Reads the watched directories. Only regular files under a directory's canonical path are read: a symbolic link (or
// Windows junction) to a directory is never followed, and a link to a file counts only when its target is inside the
// same directory (ADR-0040).

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use pathon_diag::Diagnostic;

use crate::change::{Entry, EntryKey, Kind};
use crate::config::{DirRole, SourceLimits};
use crate::{diagnostics, guard};

const EXTENSIONS: [&str; 2] = ["wgsl", "wesl"];
const MAX_DIRECTORY_DEPTH: usize = 16;
const READ_ATTEMPTS: u32 = 4;
const READ_RETRY_PAUSE: Duration = Duration::from_millis(15);

#[derive(Clone, Debug)]
pub(crate) struct WatchedDir {
    pub(crate) configured: PathBuf,
    pub(crate) canonical: PathBuf,
    pub(crate) role: DirRole,
}

/// A file as the directory sees it: where it is and, when it is acceptable, what it is called and holds.
#[derive(Debug)]
pub(crate) struct Scanned {
    pub(crate) relative: PathBuf,
    pub(crate) outcome: Result<(EntryKey, Entry), Diagnostic>,
}

pub(crate) fn scan(dir: &WatchedDir, limits: &SourceLimits) -> Vec<Scanned> {
    let mut found = Vec::new();
    walk(dir, &dir.canonical, Path::new(""), 0, limits, &mut found);
    found
}

fn walk(
    dir: &WatchedDir,
    current: &Path,
    relative: &Path,
    depth: usize,
    limits: &SourceLimits,
    found: &mut Vec<Scanned>,
) {
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let relative = relative.join(entry.file_name());
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            found.extend(scan_link(dir, &path, relative, limits));
        } else if file_type.is_dir() {
            if depth < MAX_DIRECTORY_DEPTH {
                walk(dir, &path, &relative, depth + 1, limits, found);
            }
        } else if file_type.is_file() && has_shader_extension(&path) {
            found.push(scan_file(dir, &path, relative, limits));
        }
    }
}

fn has_shader_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| EXTENSIONS.iter().any(|known| known.eq_ignore_ascii_case(extension)))
}

fn scan_link(dir: &WatchedDir, path: &Path, relative: PathBuf, limits: &SourceLimits) -> Option<Scanned> {
    let label = label_of(dir, &relative);
    let Ok(target) = fs::canonicalize(path) else {
        return None;
    };
    if !target.starts_with(&dir.canonical) {
        let diagnostic = diagnostics::outside_root(&label, "it is a link to a place outside the watched directory");
        return Some(Scanned {
            relative,
            outcome: Err(diagnostic),
        });
    }
    if target.is_dir() {
        let diagnostic = diagnostics::outside_root(&label, "links to directories are not followed");
        return Some(Scanned {
            relative,
            outcome: Err(diagnostic),
        });
    }
    has_shader_extension(path).then(|| scan_file(dir, &target, relative, limits))
}

fn scan_file(dir: &WatchedDir, path: &Path, relative: PathBuf, limits: &SourceLimits) -> Scanned {
    let label = label_of(dir, &relative);
    let outcome = read_checked(dir, path, &label, limits).and_then(|source| {
        let name = entry_name(dir.role, &relative, &label)?;
        guard::check_source(&source, limits)
            .map_err(|violation| diagnostics::too_deep(&label, violation.line, &violation.detail))?;
        Ok((
            EntryKey {
                kind: Kind::from(dir.role),
                name,
            },
            Entry {
                label,
                source: source.into(),
            },
        ))
    });
    Scanned { relative, outcome }
}

pub(crate) fn label_of(dir: &WatchedDir, relative: &Path) -> String {
    dir.configured.join(relative).display().to_string()
}

fn read_checked(dir: &WatchedDir, path: &Path, label: &str, limits: &SourceLimits) -> Result<String, Diagnostic> {
    let canonical = fs::canonicalize(path).map_err(|error| diagnostics::unreadable(label, &error.to_string()))?;
    if !canonical.starts_with(&dir.canonical) {
        return Err(diagnostics::outside_root(
            label,
            "it resolves to a place outside the watched directory",
        ));
    }
    let bytes = read_bytes(&canonical, limits.max_file_bytes, label)?;
    String::from_utf8(bytes).map_err(|_| diagnostics::unreadable(label, "it is not valid UTF-8"))
}

// An editor may hold the file for a moment while it saves, so a failed read is tried again a few times. The read is
// capped, so a file that grows after the size check is still refused instead of filling memory.
fn read_bytes(path: &Path, max_bytes: u64, label: &str) -> Result<Vec<u8>, Diagnostic> {
    let mut last_error = None;
    for attempt in 0..READ_ATTEMPTS {
        if attempt > 0 {
            thread::sleep(READ_RETRY_PAUSE);
        }
        match read_capped(path, max_bytes) {
            Ok(Capped::Bytes(bytes)) => return Ok(bytes),
            Ok(Capped::TooLarge(size)) => return Err(diagnostics::too_large(label, size, max_bytes)),
            Err(error) => last_error = Some(error),
        }
    }
    let detail = last_error.map_or_else(|| "unreadable".to_owned(), |error| error.to_string());
    Err(diagnostics::unreadable(label, &detail))
}

enum Capped {
    Bytes(Vec<u8>),
    TooLarge(u64),
}

fn read_capped(path: &Path, max_bytes: u64) -> io::Result<Capped> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    if size > max_bytes {
        return Ok(Capped::TooLarge(size));
    }
    let mut bytes = Vec::new();
    file.take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Ok(Capped::TooLarge(bytes.len() as u64));
    }
    Ok(Capped::Bytes(bytes))
}

// Roots are named by their path, modules by identifiers joined with `::`, so `import package::lighting::pbr` finds
// `lighting/pbr.wgsl`.
fn entry_name(role: DirRole, relative: &Path, label: &str) -> Result<String, Diagnostic> {
    let stem = relative.with_extension("");
    let segments: Vec<&str> = stem
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<_>>()
        .ok_or_else(|| diagnostics::unusable_name(label, "the path is not valid UTF-8"))?;
    match role {
        DirRole::Shaders => Ok(segments.join("/")),
        DirRole::Modules => {
            if let Some(bad) = segments.iter().find(|segment| !is_identifier(segment)) {
                return Err(diagnostics::unusable_name(
                    label,
                    &format!("`{bad}` is not an identifier, so it cannot be part of a module path"),
                ));
            }
            Ok(segments.join("::"))
        }
    }
}

fn is_identifier(segment: &str) -> bool {
    let mut characters = segment.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}
