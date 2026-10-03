//! Builds Rust's actual module tree by following `mod name;` declarations
//! from each crate root, as the compiler does.
//!
//! The path-based [`crate::layout`] mapping is only a starting point: it
//! cannot know that `tests/util.rs` is `mod util;` of `tests/tests.rs`
//! rather than its own test crate, that `src/cli.rs` belongs to the binary
//! when only `main.rs` declares it, or where `#[path = "..."]` points.
//! Files that no crate root reaches keep their path-based assignment.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use tree_sitter::Node;

use crate::layout::{CrateLayout, FileModule};
use crate::model::TargetKind;
use crate::symbols::syntax::{text, ItemAttributes};

/// A `mod name;` declaration (without a body) found in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModDecl {
    /// Inline modules (`mod a { mod b; }`) enclosing the declaration.
    pub inline_path: Vec<String>,
    pub name: String,
    /// Value of a `#[path = "..."]` attribute.
    pub path_attr: Option<String>,
}

/// Collects module declarations at module level (not inside functions).
pub fn declarations(root: Node<'_>, src: &str) -> Vec<ModDecl> {
    let mut out = Vec::new();
    collect(root, src, &mut Vec::new(), &mut out);
    out
}

fn collect(container: Node<'_>, src: &str, inline: &mut Vec<String>, out: &mut Vec<ModDecl>) {
    let mut cursor = container.walk();
    for item in container.named_children(&mut cursor) {
        if item.kind() != "mod_item" {
            continue;
        }
        let Some(name) = item.child_by_field_name("name") else {
            continue;
        };
        let name = text(name, src).to_string();
        match item.child_by_field_name("body") {
            Some(body) => {
                inline.push(name);
                collect(body, src, inline, out);
                inline.pop();
            }
            None => out.push(ModDecl {
                inline_path: inline.clone(),
                name,
                path_attr: ItemAttributes::from_preceding(item, src).path,
            }),
        }
    }
}

/// Reassigns files in `layout` by walking declarations from crate roots.
/// Targets whose root file turns out to be another crate's module (e.g.
/// `tests/util.rs` declared by `tests/tests.rs`) are removed.
pub fn apply(layout: &mut CrateLayout, decls: &BTreeMap<String, Vec<ModDecl>>) {
    let owned_files: HashSet<String> = layout.files.keys().cloned().collect();
    let files: HashSet<&str> = owned_files.iter().map(String::as_str).collect();
    let files = &files;
    let target_roots: HashSet<String> =
        layout.targets.iter().map(|t| t.root_file.clone()).collect();

    // Files declared as a module by some other file are never crate roots.
    let declared: HashSet<String> = decls
        .iter()
        .flat_map(|(file, file_decls)| {
            let owns_dir = owns_directory(file, target_roots.contains(file));
            file_decls
                .iter()
                .filter_map(move |d| existing_child(file, owns_dir, d, files))
        })
        .collect();

    let mut roots: Vec<(TargetKind, String, String)> = layout
        .targets
        .iter()
        .filter(|t| files.contains(t.root_file.as_str()) && !declared.contains(&t.root_file))
        .map(|t| (t.kind, t.name.clone(), t.root_file.clone()))
        .collect();
    roots.sort_by_key(|(kind, name, _)| (kind_priority(*kind), name.clone()));

    let mut assigned: HashMap<String, FileModule> = HashMap::new();
    let mut is_root: HashSet<String> = HashSet::new();
    for (_, crate_name, root_file) in &roots {
        if assigned.contains_key(root_file) {
            continue;
        }
        is_root.insert(root_file.clone());
        assigned.insert(
            root_file.clone(),
            FileModule {
                crate_name: crate_name.clone(),
                module_path: vec![crate_name.clone()],
            },
        );
        let mut queue = VecDeque::from([root_file.clone()]);
        while let Some(file) = queue.pop_front() {
            let parent = assigned[&file].clone();
            let owns_dir = owns_directory(&file, is_root.contains(&file));
            for decl in decls.get(&file).into_iter().flatten() {
                let Some(child) = existing_child(&file, owns_dir, decl, files) else {
                    continue;
                };
                if assigned.contains_key(&child) {
                    continue;
                }
                let mut module_path = parent.module_path.clone();
                module_path.extend(decl.inline_path.iter().cloned());
                module_path.push(decl.name.clone());
                assigned.insert(
                    child.clone(),
                    FileModule {
                        crate_name: parent.crate_name.clone(),
                        module_path,
                    },
                );
                queue.push_back(child);
            }
        }
    }

    for (file, module) in assigned {
        layout.files.insert(file, module);
    }
    let crates_in_use: HashSet<&str> = layout
        .files
        .values()
        .map(|m| m.crate_name.as_str())
        .collect();
    let keep: Vec<bool> = layout
        .targets
        .iter()
        .map(|t| {
            is_root.contains(&t.root_file)
                || (!files.contains(t.root_file.as_str())
                    && crates_in_use.contains(t.name.as_str()))
        })
        .collect();
    let mut keep = keep.into_iter();
    layout.targets.retain(|_| keep.next().unwrap_or(false));
}

/// Libraries first, so a file declared by both `lib.rs` and `main.rs`
/// belongs to the library.
fn kind_priority(kind: TargetKind) -> u8 {
    match kind {
        TargetKind::Lib => 0,
        TargetKind::Bin => 1,
        TargetKind::BuildScript => 2,
        TargetKind::Test => 3,
        TargetKind::Example => 4,
        TargetKind::Bench => 5,
    }
}

/// Crate roots and `mod.rs` files resolve child modules in their own
/// directory; `foo.rs` resolves them in `foo/`.
fn owns_directory(file: &str, is_root: bool) -> bool {
    is_root || file == "mod.rs" || file.ends_with("/mod.rs")
}

fn existing_child(
    file: &str,
    owns_dir: bool,
    decl: &ModDecl,
    files: &HashSet<&str>,
) -> Option<String> {
    let (dir, stem) = match file.rsplit_once('/') {
        Some((dir, name)) => (dir.to_string(), name.trim_end_matches(".rs").to_string()),
        None => (String::new(), file.trim_end_matches(".rs").to_string()),
    };
    let mut base = if owns_dir {
        dir.clone()
    } else {
        join(&dir, &stem)
    };
    for inline in &decl.inline_path {
        base = join(&base, inline);
    }

    let candidates = match &decl.path_attr {
        // `#[path]` is relative to the declaring file's directory, or to the
        // inline module's directory when nested in an inline module.
        Some(path) => {
            let origin = if decl.inline_path.is_empty() {
                &dir
            } else {
                &base
            };
            vec![normalize(&join(origin, path))]
        }
        None => vec![
            join(&base, &format!("{}.rs", decl.name)),
            join(&base, &format!("{}/mod.rs", decl.name)),
        ],
    };
    candidates.into_iter().find(|c| files.contains(c.as_str()))
}

fn join(dir: &str, rest: &str) -> String {
    if dir.is_empty() {
        rest.to_string()
    } else {
        format!("{dir}/{rest}")
    }
}

/// Resolves `.` and `..` components.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::build_layout;
    use crate::parser::RustParser;

    fn run(files: &[(&str, &str)], manifest: &str) -> CrateLayout {
        let paths: Vec<String> = files.iter().map(|(p, _)| p.to_string()).collect();
        let mut layout = build_layout("repo", &["Cargo.toml".to_string()], &paths, |_| {
            Some(manifest.to_string())
        });
        let mut parser = RustParser::new().unwrap();
        let decls = files
            .iter()
            .map(|(path, src)| {
                let tree = parser.parse(src, path).unwrap();
                (path.to_string(), declarations(tree.root_node(), src))
            })
            .collect();
        apply(&mut layout, &decls);
        layout
    }

    fn module(l: &CrateLayout, file: &str) -> String {
        l.files[file].module_path.join("::")
    }

    const PKG: &str = "[package]\nname = \"app\"\n";

    #[test]
    fn shared_test_helpers_belong_to_the_declaring_test_crate() {
        let l = run(
            &[
                ("src/lib.rs", ""),
                ("tests/tests.rs", "mod util;\nmod suite;\n"),
                ("tests/util.rs", "pub fn helper() {}\n"),
                ("tests/suite/mod.rs", "mod inner;\n"),
                ("tests/suite/inner.rs", ""),
            ],
            PKG,
        );
        assert_eq!(module(&l, "tests/util.rs"), "tests::util");
        assert_eq!(module(&l, "tests/suite/inner.rs"), "tests::suite::inner");
        let names: Vec<_> = l.targets.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["app", "tests"]);
    }

    #[test]
    fn honours_path_attributes_and_inline_modules() {
        let l = run(
            &[
                ("src/lib.rs", "mod index;\nmod outer { mod nested; }\n"),
                ("src/index/mod.rs", "#[path = \"disabled.rs\"]\nmod imp;\n"),
                ("src/index/disabled.rs", ""),
                ("src/outer/nested.rs", ""),
            ],
            PKG,
        );
        assert_eq!(module(&l, "src/index/disabled.rs"), "app::index::imp");
        assert_eq!(module(&l, "src/outer/nested.rs"), "app::outer::nested");
    }

    #[test]
    fn binary_only_modules_move_to_the_binary() {
        let l = run(
            &[
                ("src/lib.rs", "pub mod core;\n"),
                ("src/core.rs", ""),
                ("src/main.rs", "mod cli;\n"),
                ("src/cli.rs", ""),
            ],
            PKG,
        );
        assert_eq!(module(&l, "src/core.rs"), "app::core");
        assert_eq!(module(&l, "src/cli.rs"), "app_bin::cli");
    }

    #[test]
    fn normalizes_relative_paths() {
        assert_eq!(normalize("a/b/../c/./d.rs"), "a/c/d.rs");
    }
}
