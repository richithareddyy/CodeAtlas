use std::path::PathBuf;

use thiserror::Error;

pub type Result<T, E = AnalyzerError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum AnalyzerError {
    #[error("repository path does not exist or is not a directory: {0}")]
    NotADirectory(PathBuf),

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Git(#[from] GitError),

    #[error("failed to walk source tree: {0}")]
    Walk(#[from] ignore::Error),

    #[error("tree-sitter rejected the Rust grammar: {0}")]
    Grammar(#[from] tree_sitter::LanguageError),

    #[error("tree-sitter returned no tree for {0}")]
    ParseAborted(String),
}

impl AnalyzerError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        AnalyzerError::Io {
            path: path.into(),
            source,
        }
    }
}

#[derive(Debug, Error)]
pub enum GitError {
    #[error("git executable not found on PATH")]
    NotInstalled,

    #[error("`git {command}` failed ({status}): {stderr}")]
    CommandFailed {
        command: String,
        status: String,
        stderr: String,
    },

    #[error("failed to run git: {0}")]
    Spawn(std::io::Error),

    #[error("not a valid revision: `{0}`")]
    InvalidRevision(String),

    #[error("unknown revision `{0}`")]
    UnknownRevision(String),

    #[error("{path} is not inside a Git repository")]
    NotARepository { path: PathBuf },

    #[error("failed to extract revision {revision}: {message}")]
    Export { revision: String, message: String },
}
