//! Tree-sitter parsing for Rust sources.

use tree_sitter::{Node, Parser, Tree};

use crate::error::{AnalyzerError, Result};

pub struct RustParser {
    parser: Parser,
}

impl RustParser {
    pub fn new() -> Result<Self> {
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_rust::LANGUAGE.into())?;
        Ok(Self { parser })
    }

    /// Parses `source`. Tree-sitter is error-tolerant: syntactically invalid
    /// regions become ERROR nodes and the rest of the tree remains usable.
    pub fn parse(&mut self, source: &str, label: &str) -> Result<Tree> {
        self.parser
            .parse(source, None)
            .ok_or_else(|| AnalyzerError::ParseAborted(label.to_string()))
    }
}

/// Counts ERROR and MISSING nodes, i.e. regions tree-sitter had to recover from.
pub fn count_syntax_errors(root: Node<'_>) -> u32 {
    if !root.has_error() {
        return 0;
    }
    let mut count = 0;
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            count += 1;
            continue;
        }
        if node.has_error() {
            stack.extend(node.children(&mut cursor));
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_source_without_errors() {
        let mut parser = RustParser::new().unwrap();
        let tree = parser.parse("fn main() { let x = 1; }", "t").unwrap();
        assert_eq!(tree.root_node().kind(), "source_file");
        assert_eq!(count_syntax_errors(tree.root_node()), 0);
    }

    #[test]
    fn recovers_from_invalid_source() {
        let mut parser = RustParser::new().unwrap();
        let tree = parser
            .parse("fn ok() {}\nfn broken( {\nfn also_ok() {}", "t")
            .unwrap();
        assert!(count_syntax_errors(tree.root_node()) > 0);
    }
}
