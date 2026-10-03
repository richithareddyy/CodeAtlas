//! Helpers that read structured information out of tree-sitter nodes.

use tree_sitter::Node;

use crate::model::{PathSegments, Visibility};

pub fn text<'s>(node: Node<'_>, src: &'s str) -> &'s str {
    &src[node.byte_range()]
}

/// 1-based line of the node's first character.
pub fn start_line(node: Node<'_>) -> u32 {
    node.start_position().row as u32 + 1
}

/// 1-based line of the node's last character.
pub fn end_line(node: Node<'_>) -> u32 {
    node.end_position().row as u32 + 1
}

/// Path segments of a path-like node with generic arguments removed.
/// Returns `None` for nodes that are not paths (references, tuples, ...).
pub fn path_segments(node: Node<'_>, src: &str) -> Option<PathSegments> {
    match node.kind() {
        "identifier" | "type_identifier" | "field_identifier" | "primitive_type" | "self"
        | "super" | "crate" | "metavariable" => Some(vec![text(node, src).to_string()]),
        "scoped_identifier" | "scoped_type_identifier" => {
            let mut segments = match node.child_by_field_name("path") {
                Some(path) => path_segments(path, src)?,
                // `::name` — a global path.
                None if text(node, src).starts_with("::") => vec![String::new()],
                None => vec![],
            };
            segments.push(text(node.child_by_field_name("name")?, src).to_string());
            Some(segments)
        }
        "generic_type" | "generic_function" => {
            let inner = node
                .child_by_field_name("type")
                .or_else(|| node.child_by_field_name("function"))?;
            path_segments(inner, src)
        }
        _ => None,
    }
}

/// Like [`path_segments`] but falls back to the whitespace-normalised source
/// text for types that are not paths (`[T]`, `&str`, `(A, B)`).
pub fn type_segments(node: Node<'_>, src: &str) -> PathSegments {
    path_segments(node, src).unwrap_or_else(|| vec![normalize_ws(text(node, src))])
}

pub fn visibility(item: Node<'_>, src: &str) -> Visibility {
    let mut cursor = item.walk();
    let Some(modifier) = item
        .children(&mut cursor)
        .find(|c| c.kind() == "visibility_modifier")
    else {
        return Visibility::Private;
    };
    let raw: String = text(modifier, src).split_whitespace().collect();
    match raw.as_str() {
        "pub" => Visibility::Public,
        "pub(crate)" | "crate" => Visibility::Crate,
        "pub(super)" => Visibility::Super,
        "pub(self)" => Visibility::Private,
        other => {
            let path = other
                .strip_prefix("pub(in")
                .and_then(|p| p.strip_suffix(')'))
                .unwrap_or(other);
            Visibility::Restricted(path.to_string())
        }
    }
}

/// Item text up to (not including) its body, whitespace-normalised.
/// For `pub fn f(x: u8) -> u8 { .. }` this yields `pub fn f(x: u8) -> u8`.
pub fn signature(item: Node<'_>, src: &str) -> String {
    let end = item
        .child_by_field_name("body")
        .map(|body| body.start_byte())
        .unwrap_or_else(|| item.end_byte());
    let raw = &src[item.start_byte()..end];
    normalize_ws(raw.trim_end_matches(';'))
}

pub fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Attributes that precede an item, collected from `attribute_item` siblings.
#[derive(Debug, Default, Clone, Copy)]
pub struct ItemAttributes {
    pub is_test: bool,
    pub cfg_test: bool,
}

impl ItemAttributes {
    pub fn from_preceding(item: Node<'_>, src: &str) -> Self {
        let mut attrs = Self::default();
        let mut sibling = item.prev_sibling();
        while let Some(node) = sibling {
            match node.kind() {
                "attribute_item" => {
                    if let Some(attr) = node.named_child(0) {
                        attrs.apply(attr, src);
                    }
                }
                "line_comment" | "block_comment" => {}
                _ => break,
            }
            sibling = node.prev_sibling();
        }
        attrs
    }

    /// Inner attributes (`#![cfg(test)]`) at the top of a file or module body.
    pub fn from_inner(container: Node<'_>, src: &str) -> Self {
        let mut attrs = Self::default();
        let mut cursor = container.walk();
        for node in container.children(&mut cursor) {
            if node.kind() == "inner_attribute_item" {
                if let Some(attr) = node.named_child(0) {
                    attrs.apply(attr, src);
                }
            }
        }
        attrs
    }

    fn apply(&mut self, attribute: Node<'_>, src: &str) {
        let Some(path) = attribute.named_child(0) else {
            return;
        };
        let path = text(path, src);
        let args = attribute
            .child_by_field_name("arguments")
            .map(|a| text(a, src))
            .unwrap_or("");
        if is_test_attribute(path) {
            self.is_test = true;
        }
        if path == "cfg" && cfg_enables_test(args) {
            self.cfg_test = true;
        }
    }
}

/// `#[test]`, `#[tokio::test]`, `#[async_std::test]`, `#[rstest]`, `#[test_case(..)]`.
fn is_test_attribute(path: &str) -> bool {
    path == "test"
        || path.ends_with("::test")
        || matches!(path, "rstest" | "test_case" | "quickcheck")
}

/// True when a `cfg(...)` predicate mentions `test` outside a `not(...)`.
/// This is a conservative approximation of cfg evaluation.
fn cfg_enables_test(args: &str) -> bool {
    let compact: String = args.split_whitespace().collect();
    let mentions_test = compact
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|token| token == "test");
    mentions_test && !compact.contains("not(test")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_test_attributes() {
        assert!(is_test_attribute("test"));
        assert!(is_test_attribute("tokio::test"));
        assert!(is_test_attribute("rstest"));
        assert!(!is_test_attribute("testing"));
        assert!(!is_test_attribute("derive"));
    }

    #[test]
    fn evaluates_cfg_test_conservatively() {
        assert!(cfg_enables_test("(test)"));
        assert!(cfg_enables_test("(all(test, feature = \"x\"))"));
        assert!(!cfg_enables_test("(not(test))"));
        assert!(!cfg_enables_test("(feature = \"testing\")"));
    }
}
