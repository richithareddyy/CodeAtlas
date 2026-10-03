//! Maps Rust source files onto Cargo targets and module paths.
//!
//! The mapping follows Cargo's default target layout and Rust's file-module
//! conventions (`a.rs` / `a/mod.rs`). It is path-based: `#[path]` attributes
//! and modules that are never declared with `mod` are not detected. When a
//! package has both `src/lib.rs` and `src/main.rs`, files under `src/` are
//! attributed to the library and the binary root is named `<package>_bin`.
//! Targets with explicit, non-conventional `path` keys in `Cargo.toml`
//! (`[lib]`, `[[bin]]`, `[[test]]`, ...) take precedence over the defaults.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use crate::model::{CrateTarget, TargetKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileModule {
    pub crate_name: String,
    /// Full module path, starting with the crate name.
    pub module_path: Vec<String>,
}

#[derive(Debug, Default)]
pub struct CrateLayout {
    pub targets: Vec<CrateTarget>,
    /// Keyed by repository-relative file path.
    pub files: BTreeMap<String, FileModule>,
    /// Crate names of declared dependencies (normalised, from every
    /// manifest's `[*dependencies]` tables). A path rooted at one of these
    /// names that does not resolve inside the repository is external.
    pub dependencies: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct Package {
    /// Package directory relative to the repository root ("" for the root).
    dir: String,
    name: String,
    /// Targets declared with a non-conventional `path`.
    explicit: Vec<ExplicitTarget>,
}

#[derive(Debug, Clone)]
struct ExplicitTarget {
    name: String,
    kind: TargetKind,
    /// Root file relative to the package directory.
    root: String,
    /// Declared at Cargo's default location (only the name is explicit).
    conventional: bool,
}

impl ExplicitTarget {
    /// Directory whose files belong to this target's module tree.
    fn dir(&self) -> &str {
        self.root.rsplit_once('/').map_or("", |(dir, _)| dir)
    }
}

/// Builds the layout for `rust_files`. `read_manifest` returns the contents
/// of a repository-relative `Cargo.toml`; injecting it keeps this module free
/// of filesystem access and easy to test.
pub fn build_layout(
    repo_name: &str,
    manifests: &[String],
    rust_files: &[String],
    read_manifest: impl Fn(&str) -> Option<String>,
) -> CrateLayout {
    let mut dependencies = BTreeSet::new();
    let mut packages: Vec<Package> = manifests
        .iter()
        .filter_map(|manifest| {
            let content = read_manifest(manifest)?;
            dependencies.extend(dependency_names(&content));
            let (name, explicit) = parse_manifest(&content)?;
            let dir = manifest
                .strip_suffix("Cargo.toml")
                .unwrap_or("")
                .trim_end_matches('/')
                .to_string();
            Some(Package {
                dir,
                name,
                explicit,
            })
        })
        .collect();
    // Deepest directory first so nested packages win the prefix match.
    packages.sort_by_key(|p| std::cmp::Reverse(p.dir.len()));

    let file_set: HashSet<&str> = rust_files.iter().map(String::as_str).collect();
    let mut layout = CrateLayout::default();
    let mut targets: BTreeMap<String, CrateTarget> = BTreeMap::new();

    for file in rust_files {
        let (package_name, explicit, rel) = match owning_package(&packages, file) {
            Some((pkg, rel)) => (pkg.name.clone(), pkg.explicit.as_slice(), rel),
            None => (repo_name.to_string(), &[][..], file.as_str()),
        };
        let pkg_dir = &file[..file.len() - rel.len()];
        let has = |p: &str| file_set.contains(format!("{pkg_dir}{p}").as_str());
        let has_lib = has("src/lib.rs");
        let has_main = has("src/main.rs");

        let (target_name, kind, root_rel, module_rel) = match classify_explicit(explicit, rel) {
            Some(found) => found,
            None => classify(&package_name, rel, has_lib, has_main),
        };

        let crate_name = normalize(&target_name);
        let mut module_path = vec![crate_name.clone()];
        module_path.extend(module_rel);

        targets
            .entry(format!("{}::{}", package_name, crate_name))
            .or_insert_with(|| CrateTarget {
                name: crate_name.clone(),
                package: package_name.clone(),
                kind,
                root_file: format!("{pkg_dir}{root_rel}"),
            });
        layout.files.insert(
            file.clone(),
            FileModule {
                crate_name,
                module_path,
            },
        );
    }

    layout.targets = targets.into_values().collect();
    layout.dependencies = dependencies;
    layout
}

fn owning_package<'p, 'f>(
    packages: &'p [Package],
    file: &'f str,
) -> Option<(&'p Package, &'f str)> {
    packages.iter().find_map(|pkg| {
        if pkg.dir.is_empty() {
            Some((pkg, file))
        } else {
            file.strip_prefix(pkg.dir.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
                .map(|rest| (pkg, rest))
        }
    })
}

type Classification = (String, TargetKind, String, Vec<String>);

/// Matches a file against explicitly declared targets: the target's root file
/// itself, or the deepest target directory containing the file.
fn classify_explicit(explicit: &[ExplicitTarget], rel: &str) -> Option<Classification> {
    if let Some(target) = explicit.iter().find(|t| t.root == rel) {
        return Some((
            target.name.clone(),
            target.kind,
            target.root.clone(),
            vec![],
        ));
    }
    let (target, rest) = explicit
        .iter()
        .filter(|t| !t.conventional && !t.dir().is_empty())
        .filter_map(|t| {
            let rest = rel.strip_prefix(t.dir())?.strip_prefix('/')?;
            Some((t, rest))
        })
        .max_by_key(|(t, _)| t.dir().len())?;
    let parts: Vec<&str> = rest.split('/').collect();
    Some((
        target.name.clone(),
        target.kind,
        target.root.clone(),
        module_segments(&parts, false),
    ))
}

/// Returns (target name, kind, target root relative to package, module path
/// segments within the target) for a package-relative file path.
fn classify(package: &str, rel: &str, has_lib: bool, has_main: bool) -> Classification {
    let parts: Vec<&str> = rel.split('/').collect();

    match parts.as_slice() {
        ["build.rs"] => (
            format!("{package}_build"),
            TargetKind::BuildScript,
            rel.into(),
            vec![],
        ),
        ["src", "main.rs"] => {
            let name = if has_lib {
                format!("{package}_bin")
            } else {
                package.into()
            };
            (name, TargetKind::Bin, rel.into(), vec![])
        }
        ["src", "bin", rest @ ..] => nested_target(rest, TargetKind::Bin, "src/bin"),
        [dir @ ("tests" | "examples" | "benches"), rest @ ..] => {
            let kind = match *dir {
                "tests" => TargetKind::Test,
                "examples" => TargetKind::Example,
                _ => TargetKind::Bench,
            };
            nested_target(rest, kind, dir)
        }
        ["src", rest @ ..] => {
            let (name, kind, root) = if has_lib || !has_main {
                (package.to_string(), TargetKind::Lib, "src/lib.rs")
            } else {
                (package.to_string(), TargetKind::Bin, "src/main.rs")
            };
            (name, kind, root.into(), module_segments(rest, true))
        }
        // Files outside Cargo's conventional directories (or loose files in a
        // repository without Cargo.toml) are modelled as modules of the package.
        rest => (
            package.into(),
            TargetKind::Lib,
            String::new(),
            module_segments(rest, false),
        ),
    }
}

/// Targets rooted at `<dir>/<name>.rs` or `<dir>/<name>/main.rs`.
fn nested_target(
    rest: &[&str],
    kind: TargetKind,
    dir: &str,
) -> (String, TargetKind, String, Vec<String>) {
    match rest {
        [file] => {
            let name = file.trim_end_matches(".rs").to_string();
            let root = format!("{dir}/{file}");
            (name, kind, root, vec![])
        }
        [name, inner @ ..] => {
            let root = format!("{dir}/{name}/main.rs");
            (name.to_string(), kind, root, module_segments(inner, true))
        }
        [] => (dir.into(), kind, dir.into(), vec![]),
    }
}

/// Converts path components into module segments: `a/b.rs` → `[a, b]`,
/// `a/mod.rs` → `[a]`; crate-root files (`lib.rs`, `main.rs`) map to `[]`
/// when `at_target_root` is set.
fn module_segments(parts: &[&str], at_target_root: bool) -> Vec<String> {
    let mut segments: Vec<String> = parts.iter().map(|p| p.to_string()).collect();
    if let Some(last) = segments.pop() {
        let stem = last.strip_suffix(".rs").unwrap_or(&last);
        let is_root = at_target_root && segments.is_empty() && matches!(stem, "lib" | "main");
        if stem != "mod" && !is_root {
            segments.push(stem.to_string());
        }
    }
    segments.into_iter().map(|s| normalize(&s)).collect()
}

/// Cargo replaces `-` with `_` in crate names; module names cannot contain it.
fn normalize(name: &str) -> String {
    name.replace('-', "_")
}

/// Package name and explicitly located targets, or `None` for manifests
/// without a `[package]` (virtual workspace roots) or invalid TOML.
fn parse_manifest(manifest: &str) -> Option<(String, Vec<ExplicitTarget>)> {
    let value: toml::Table = toml::from_str(manifest).ok()?;
    let name = value.get("package")?.get("name")?.as_str()?.to_string();

    let mut explicit = Vec::new();
    let mut push = |table: &toml::Value, kind: TargetKind, default_name: &str| {
        let Some(path) = table.get("path").and_then(|p| p.as_str()) else {
            return;
        };
        let path = path.trim_start_matches("./").to_string();
        let conventional = is_conventional(kind, &path);
        let name = table
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or(default_name);
        explicit.push(ExplicitTarget {
            name: name.to_string(),
            kind,
            root: path,
            conventional,
        });
    };

    if let Some(lib) = value.get("lib") {
        push(lib, TargetKind::Lib, &name);
    }
    for (key, kind) in [
        ("bin", TargetKind::Bin),
        ("test", TargetKind::Test),
        ("example", TargetKind::Example),
        ("bench", TargetKind::Bench),
    ] {
        for table in value
            .get(key)
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            push(table, kind, &name);
        }
    }
    Some((name, explicit))
}

/// Keys of `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`,
/// their `[target.*]` variants and `[workspace.dependencies]`.
fn dependency_names(manifest: &str) -> Vec<String> {
    let Ok(value) = toml::from_str::<toml::Table>(manifest) else {
        return Vec::new();
    };
    let tables = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut sections: Vec<&toml::Value> = tables.iter().filter_map(|t| value.get(*t)).collect();
    if let Some(targets) = value.get("target").and_then(|t| t.as_table()) {
        for target in targets.values() {
            sections.extend(tables.iter().filter_map(|t| target.get(*t)));
        }
    }
    if let Some(deps) = value.get("workspace").and_then(|w| w.get("dependencies")) {
        sections.push(deps);
    }
    sections
        .into_iter()
        .filter_map(|s| s.as_table())
        .flat_map(|t| t.keys())
        .map(|k| normalize(k))
        .collect()
}

fn is_conventional(kind: TargetKind, path: &str) -> bool {
    match kind {
        TargetKind::Lib => path == "src/lib.rs",
        TargetKind::Bin => path == "src/main.rs" || path.starts_with("src/bin/"),
        TargetKind::Test => path.starts_with("tests/"),
        TargetKind::Example => path.starts_with("examples/"),
        TargetKind::Bench => path.starts_with("benches/"),
        TargetKind::BuildScript => path == "build.rs",
    }
}

/// Reads a manifest relative to `root`; missing or unreadable manifests are
/// treated as absent.
pub fn manifest_reader(root: &Path) -> impl Fn(&str) -> Option<String> + '_ {
    move |rel| std::fs::read_to_string(root.join(rel)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(files: &[&str], manifests: &[(&str, &str)]) -> CrateLayout {
        let files: Vec<String> = files.iter().map(|s| s.to_string()).collect();
        let manifest_paths: Vec<String> = manifests.iter().map(|(p, _)| p.to_string()).collect();
        build_layout("repo", &manifest_paths, &files, |p| {
            manifests
                .iter()
                .find(|(path, _)| *path == p)
                .map(|(_, content)| content.to_string())
        })
    }

    fn module(l: &CrateLayout, file: &str) -> String {
        l.files[file].module_path.join("::")
    }

    const PKG: &str = "[package]\nname = \"my-app\"\n";

    #[test]
    fn maps_library_modules() {
        let l = layout(
            &["src/lib.rs", "src/a.rs", "src/a/b.rs", "src/c/mod.rs"],
            &[("Cargo.toml", PKG)],
        );
        assert_eq!(module(&l, "src/lib.rs"), "my_app");
        assert_eq!(module(&l, "src/a.rs"), "my_app::a");
        assert_eq!(module(&l, "src/a/b.rs"), "my_app::a::b");
        assert_eq!(module(&l, "src/c/mod.rs"), "my_app::c");
        assert_eq!(l.targets.len(), 1);
        assert_eq!(l.targets[0].kind, TargetKind::Lib);
    }

    #[test]
    fn separates_bin_test_and_example_targets() {
        let l = layout(
            &[
                "src/lib.rs",
                "src/main.rs",
                "src/bin/tool.rs",
                "tests/api.rs",
                "tests/common/mod.rs",
                "examples/demo.rs",
            ],
            &[("Cargo.toml", PKG)],
        );
        assert_eq!(module(&l, "src/main.rs"), "my_app_bin");
        assert_eq!(module(&l, "src/bin/tool.rs"), "tool");
        assert_eq!(module(&l, "tests/api.rs"), "api");
        assert_eq!(module(&l, "tests/common/mod.rs"), "common");
        assert_eq!(module(&l, "examples/demo.rs"), "demo");
        let kinds: Vec<_> = l
            .targets
            .iter()
            .map(|t| (t.name.as_str(), t.kind))
            .collect();
        assert!(kinds.contains(&("api", TargetKind::Test)));
        assert!(kinds.contains(&("my_app_bin", TargetKind::Bin)));
    }

    #[test]
    fn binary_only_package_owns_src_modules() {
        let l = layout(&["src/main.rs", "src/cli.rs"], &[("Cargo.toml", PKG)]);
        assert_eq!(module(&l, "src/main.rs"), "my_app");
        assert_eq!(module(&l, "src/cli.rs"), "my_app::cli");
        assert_eq!(l.targets[0].kind, TargetKind::Bin);
    }

    #[test]
    fn collects_dependency_names_and_explicit_names_at_default_paths() {
        let l = layout(
            &["src/lib.rs", "tests/tests.rs"],
            &[(
                "Cargo.toml",
                "[package]\nname = \"app\"\n[dependencies]\nserde-json = \"1\"\n[dev-dependencies]\nglob = \"0.3\"\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n[[test]]\nname = \"integration\"\npath = \"tests/tests.rs\"\n",
            )],
        );
        let deps: Vec<_> = l.dependencies.iter().map(String::as_str).collect();
        assert_eq!(deps, vec!["glob", "libc", "serde_json"]);
        assert_eq!(module(&l, "tests/tests.rs"), "integration");
    }

    #[test]
    fn honours_explicit_target_paths() {
        let l = layout(
            &[
                "crates/core/main.rs",
                "crates/core/flags/mod.rs",
                "crates/core/flags/parse.rs",
                "src/lib.rs",
            ],
            &[(
                "Cargo.toml",
                "[package]\nname = \"ripgrep\"\n[lib]\npath = \"src/lib.rs\"\n[[bin]]\nname = \"rg\"\npath = \"crates/core/main.rs\"\n",
            )],
        );
        assert_eq!(module(&l, "crates/core/main.rs"), "rg");
        assert_eq!(module(&l, "crates/core/flags/mod.rs"), "rg::flags");
        assert_eq!(module(&l, "crates/core/flags/parse.rs"), "rg::flags::parse");
        assert_eq!(module(&l, "src/lib.rs"), "ripgrep");
        let rg = l.targets.iter().find(|t| t.name == "rg").unwrap();
        assert_eq!(
            (rg.kind, rg.root_file.as_str()),
            (TargetKind::Bin, "crates/core/main.rs")
        );
    }

    #[test]
    fn resolves_nested_workspace_packages() {
        let l = layout(
            &[
                "crates/core/src/lib.rs",
                "crates/core/src/x.rs",
                "tools/gen.rs",
            ],
            &[
                ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
                ("crates/core/Cargo.toml", "[package]\nname = \"core-lib\"\n"),
            ],
        );
        assert_eq!(module(&l, "crates/core/src/x.rs"), "core_lib::x");
        assert_eq!(l.targets[0].root_file, "crates/core/src/lib.rs");
        // No package owns tools/: falls back to the repository name.
        assert_eq!(module(&l, "tools/gen.rs"), "repo::tools::gen");
    }
}
