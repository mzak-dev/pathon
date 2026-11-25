use std::io;
use std::path::PathBuf;

/// Why hot reload could not start; everything after the start is a diagnostic in the [`crate::HotReport`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HotError {
    #[error("no directory to watch was configured")]
    NoDirectories,
    #[error("cannot watch `{}`: {source}", path.display())]
    DirectoryUnavailable { path: PathBuf, source: io::Error },
    #[error("the file watcher could not be started: {0}")]
    Watcher(#[source] notify::Error),
    #[error("the hot reload thread could not be started: {0}")]
    Thread(#[source] io::Error),
}
