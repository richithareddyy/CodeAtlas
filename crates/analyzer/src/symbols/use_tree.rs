//! Flattens `use` trees into one entry per imported name.
//!
//! `use crate::a::{self, b as c, d::*};` becomes
//! `crate::a`, `crate::a::b as c` and `crate::a::d::*`.

use tree_sitter::Node;

use super::syntax::{path_segments, text};
use crate::model::PathSegments;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseItem {
    pub path: PathSegments,
    pub alias: Option<String>,
    pub glob: bool,
}

pub fn flatten_use(declaration: Node<'_>, src: &str) -> Vec<UseItem> {
    let mut items = Vec::new();
    if let Some(argument) = declaration.child_by_field_name("argument") {
        flatten(argument, src, &[], &mut items);
    }
    items
}

fn flatten(node: Node<'_>, src: &str, prefix: &[String], out: &mut Vec<UseItem>) {
    match node.kind() {
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                flatten(child, src, prefix, out);
            }
        }
        "scoped_use_list" => {
            let mut new_prefix = prefix.to_vec();
            if let Some(path) = node.child_by_field_name("path") {
                new_prefix.extend(path_segments(path, src).unwrap_or_default());
            } else if text(node, src).starts_with("::") {
                new_prefix.push(String::new());
            }
            if let Some(list) = node.child_by_field_name("list") {
                flatten(list, src, &new_prefix, out);
            }
        }
        "use_as_clause" => {
            let Some(path) = node.child_by_field_name("path") else {
                return;
            };
            let alias = node
                .child_by_field_name("alias")
                .map(|a| text(a, src).to_string());
            out.push(UseItem {
                path: join(prefix, path_segments(path, src).unwrap_or_default()),
                alias,
                glob: false,
            });
        }
        "use_wildcard" => {
            let inner = node
                .named_child(0)
                .and_then(|p| path_segments(p, src))
                .unwrap_or_default();
            out.push(UseItem {
                path: join(prefix, inner),
                alias: None,
                glob: true,
            });
        }
        // `self` inside a list imports the prefix module itself.
        "self" if !prefix.is_empty() => out.push(UseItem {
            path: prefix.to_vec(),
            alias: None,
            glob: false,
        }),
        _ => {
            if let Some(segments) = path_segments(node, src) {
                out.push(UseItem {
                    path: join(prefix, segments),
                    alias: None,
                    glob: false,
                });
            }
        }
    }
}

fn join(prefix: &[String], rest: PathSegments) -> PathSegments {
    let mut path = prefix.to_vec();
    path.extend(rest);
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::RustParser;

    fn items(src: &str) -> Vec<(String, Option<String>, bool)> {
        let mut parser = RustParser::new().unwrap();
        let tree = parser.parse(src, "t").unwrap();
        let decl = tree.root_node().named_child(0).unwrap();
        flatten_use(decl, src)
            .into_iter()
            .map(|i| (i.path.join("::"), i.alias, i.glob))
            .collect()
    }

    #[test]
    fn flattens_simple_paths() {
        assert_eq!(items("use a::b::c;"), vec![("a::b::c".into(), None, false)]);
        assert_eq!(items("use std;"), vec![("std".into(), None, false)]);
    }

    #[test]
    fn flattens_nested_groups_aliases_and_globs() {
        assert_eq!(
            items("use crate::a::{self, b as c, d::*, e::{f, g}};"),
            vec![
                ("crate::a".into(), None, false),
                ("crate::a::b".into(), Some("c".into()), false),
                ("crate::a::d".into(), None, true),
                ("crate::a::e::f".into(), None, false),
                ("crate::a::e::g".into(), None, false),
            ]
        );
    }

    #[test]
    fn handles_relative_and_global_paths() {
        assert_eq!(
            items("use super::x as _;"),
            vec![("super::x".into(), Some("_".into()), false)]
        );
        assert_eq!(
            items("use self::m::*;"),
            vec![("self::m".into(), None, true)]
        );
        assert_eq!(
            items("use ::ext::Item;"),
            vec![("::ext::Item".into(), None, false)]
        );
    }
}
