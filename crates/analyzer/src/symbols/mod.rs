//! Symbol extraction from Rust syntax trees.

mod extractor;
pub mod syntax;
pub(crate) mod use_tree;

pub use extractor::{extract_file, FileContext};

#[cfg(test)]
mod tests;
