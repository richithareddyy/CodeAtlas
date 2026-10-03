//! CodeAtlas analysis core.
//!
//! Pipeline: [`ingest`] a repository → map files onto crates and modules
//! ([`layout`]) → [`parser`] → [`symbols`] extraction → [`analysis`] assembly.
//! Nothing in this crate talks to a database; persistence lives in the
//! store crate.

pub mod analysis;
pub mod error;
pub mod evaluation;
pub mod git;
pub mod ingest;
pub mod layout;
pub mod model;
pub mod module_tree;
pub mod parser;
pub mod resolver;
pub mod symbols;

pub use analysis::{analyze, analyze_source, AnalysisStats, RepositoryAnalysis};
pub use error::{AnalyzerError, Result};
pub use ingest::{ingest, IngestOptions, RepoSource, RepositoryInfo};
