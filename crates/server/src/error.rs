//! Maps domain errors onto GraphQL errors with a machine-readable
//! `extensions.code`.
//!
//! | code | meaning |
//! |---|---|
//! | `NOT_FOUND` | unknown repository, symbol or file |
//! | `BAD_USER_INPUT` | invalid argument (depth out of range, empty search, ...) |
//! | `AMBIGUOUS` | a name matches several symbols or repositories |
//! | `OUTDATED_INDEX` | the repository must be indexed again |
//! | `FORBIDDEN` | the operation is disabled by configuration |
//! | `INDEX_FAILED` | analysing a repository failed |
//! | `NOT_A_GIT_REPOSITORY` | Git operations on a repository without Git history |
//! | `SOURCE_UNAVAILABLE` | the repository's files are not readable on the server |
//! | `DIFF_FAILED` | comparing two revisions failed |
//! | `INTERNAL` | anything else; details are logged, not returned |

use async_graphql::{Error, ErrorExtensions};
use codeatlas_analyzer::error::{AnalyzerError, GitError};
use codeatlas_analyzer::graph::impact::ImpactError;
use codeatlas_store::StoreError;

pub fn coded(message: impl Into<String>, code: &'static str) -> Error {
    Error::new(message.into()).extend_with(|_, e| e.set("code", code))
}

pub fn bad_input(message: impl Into<String>) -> Error {
    coded(message, "BAD_USER_INPUT")
}

pub fn internal(error: impl std::fmt::Display) -> Error {
    tracing::error!(%error, "request failed");
    coded("internal error; see the server log", "INTERNAL")
}

pub fn from_store(error: StoreError) -> Error {
    match error {
        StoreError::NotFound(_) => coded(error.to_string(), "NOT_FOUND"),
        StoreError::InvalidArgument(_) => coded(error.to_string(), "BAD_USER_INPUT"),
        StoreError::Ambiguous(_) => coded(error.to_string(), "AMBIGUOUS"),
        StoreError::OutdatedIndex { .. } => coded(error.to_string(), "OUTDATED_INDEX"),
        StoreError::Config(_) | StoreError::Neo4j(_) | StoreError::Decode(_) => internal(error),
    }
}

pub fn from_impact(error: ImpactError) -> Error {
    match error {
        ImpactError::UnknownSymbol(_) | ImpactError::UnknownFile(_) => {
            coded(error.to_string(), "NOT_FOUND")
        }
        ImpactError::NothingChanged => coded(error.to_string(), "BAD_USER_INPUT"),
    }
}

/// Errors from Git operations and diff analysis.
pub fn from_diff(error: AnalyzerError) -> Error {
    let code = match &error {
        AnalyzerError::Git(GitError::InvalidRevision(_)) => "BAD_USER_INPUT",
        AnalyzerError::Git(GitError::UnknownRevision(_)) => "NOT_FOUND",
        AnalyzerError::Git(GitError::NotARepository { .. }) => "NOT_A_GIT_REPOSITORY",
        AnalyzerError::NotADirectory(_) => "SOURCE_UNAVAILABLE",
        _ => "DIFF_FAILED",
    };
    coded(error.to_string(), code)
}

/// `?`-friendly conversion for resolvers.
pub trait StoreResultExt<T> {
    fn gql(self) -> async_graphql::Result<T>;
}

impl<T> StoreResultExt<T> for Result<T, StoreError> {
    fn gql(self) -> async_graphql::Result<T> {
        self.map_err(from_store)
    }
}
