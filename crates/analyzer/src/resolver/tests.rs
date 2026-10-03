//! Rule-level resolver tests on small in-memory crates.

use super::Resolution;
use crate::analysis::assemble;
use crate::model::{CrateTarget, EdgeKind, FileAnalysis, TargetKind};
use crate::parser::RustParser;
use crate::symbols::{extract_file, FileContext};

/// Builds a single-crate `app` from `(file path, module path, source)`.
fn resolve(files: &[(&str, &str, &str)]) -> (Vec<FileAnalysis>, Resolution) {
    let mut parser = RustParser::new().unwrap();
    let mut analyses: Vec<FileAnalysis> = files
        .iter()
        .map(|(path, module, src)| {
            let module_path: Vec<String> = module.split("::").map(String::from).collect();
            let tree = parser.parse(src, path).unwrap();
            let ctx = FileContext {
                path,
                crate_name: "app",
                module_path: &module_path,
            };
            extract_file(&ctx, src, &tree, &mut parser)
        })
        .collect();
    let crates = vec![CrateTarget {
        name: "app".into(),
        package: "app".into(),
        kind: TargetKind::Lib,
        root_file: "src/lib.rs".into(),
    }];
    let resolution = assemble(&mut analyses, &crates, &Default::default());
    (analyses, resolution)
}

fn calls(r: &Resolution) -> Vec<String> {
    r.edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .map(|e| format!("{} -> {}", e.from, e.to))
        .collect()
}

fn single(src: &str) -> Resolution {
    resolve(&[("src/lib.rs", "app", src)]).1
}

#[test]
fn walks_super_chains_and_self_paths() {
    let r = single(
        "fn top() {}\nmod a {\n    pub fn sibling() {}\n    pub mod b {\n        fn f() { super::super::top(); self::g(); super::sibling(); crate::top(); }\n        fn g() {}\n    }\n}\n",
    );
    assert_eq!(
        calls(&r),
        vec![
            "fn:app::a::b::f -> fn:app::a::b::g",
            "fn:app::a::b::f -> fn:app::a::sibling",
            "fn:app::a::b::f -> fn:app::top",
        ]
    );
}

#[test]
fn modules_do_not_inherit_names_from_parents() {
    let r = single("fn helper() {}\nmod child {\n    fn f() { helper(); }\n}\n");
    assert!(calls(&r).is_empty());
    assert_eq!(r.unresolved_calls.len(), 1);
    assert_eq!(r.unresolved_calls[0].reason.as_str(), "name_not_in_scope");
}

#[test]
fn only_pub_use_is_visible_through_paths() {
    let r = single(
        "mod inner { pub fn target() {} }\nmod reexport { pub use crate::inner::target; }\nmod private { use crate::inner::target; }\nfn f() { reexport::target(); private::target(); }\n",
    );
    assert_eq!(calls(&r), vec!["fn:app::f -> fn:app::inner::target"]);
    assert_eq!(r.unresolved_calls[0].callee, "private::target");
    assert_eq!(
        r.unresolved_calls[0].reason.as_str(),
        "path_segment_not_found"
    );
}

#[test]
fn local_items_shadow_explicit_imports_which_shadow_globs() {
    let r = single(
        "mod a { pub fn run() {} pub fn only_glob() {} }\nmod b { pub fn run() {} }\nmod c {\n    use crate::a::*;\n    use crate::b::run;\n    fn f() { run(); only_glob(); }\n}\nmod d {\n    use crate::a::run;\n    fn run() {}\n    fn g() { run(); }\n}\n",
    );
    assert_eq!(
        calls(&r),
        vec![
            "fn:app::c::f -> fn:app::a::only_glob",
            "fn:app::c::f -> fn:app::b::run",
            "fn:app::d::g -> fn:app::d::run",
        ]
    );
}

#[test]
fn classifies_constructors_and_external_calls() {
    let r = single(
        "pub struct Meters(u32);\npub enum Shape { Circle(u32) }\nfn f() { Meters(1); Shape::Circle(2); Some(3); String::from(\"x\"); vec![1].len(); }\n",
    );
    assert!(calls(&r).is_empty());
    let stats = &r.stats.calls;
    assert_eq!(stats.constructors, 2);
    assert_eq!(stats.external, 3);
    assert_eq!(stats.resolution_rate, None);
}

#[test]
fn infers_receivers_from_bindings_fields_and_returns() {
    let r = single(
        r#"
use std::sync::Arc;
pub struct Repo;
impl Repo {
    pub fn open() -> Result<Self, String> { Ok(Repo) }
    pub fn save(&self) {}
}
pub struct Service { repo: Arc<Repo> }
impl Service {
    fn run(&self) { self.repo.save(); }
}
fn with_try() -> Result<(), String> {
    let repo = Repo::open()?;
    repo.save();
    Ok(())
}
fn with_literal() {
    let service = Service { repo: Arc::new(Repo) };
    service.run();
}
"#,
    );
    assert_eq!(
        calls(&r),
        vec![
            "fn:app::with_literal -> method:app::Service::run",
            "fn:app::with_try -> method:app::Repo::open",
            "fn:app::with_try -> method:app::Repo::save",
            "method:app::Service::run -> method:app::Repo::save",
        ]
    );
}

#[test]
fn uses_the_binding_in_effect_at_the_call_line() {
    let r = single(
        "pub struct A;\nimpl A { pub fn go(&self) {} }\npub struct B;\nimpl B { pub fn go(&self) {} }\nfn f() {\n    let x = A {};\n    x.go();\n    let x = B {};\n    x.go();\n}\n",
    );
    assert_eq!(
        calls(&r),
        vec![
            "fn:app::f -> method:app::A::go",
            "fn:app::f -> method:app::B::go"
        ]
    );
}

#[test]
fn resolves_qualified_trait_calls_to_the_implementation() {
    let r = single(
        "pub trait Speak { fn speak(&self); }\npub struct Dog;\nimpl Speak for Dog { fn speak(&self) {} }\nfn f(d: Dog) { <Dog as Speak>::speak(&d); Speak::speak(&d); }\n",
    );
    assert_eq!(
        calls(&r),
        vec![
            "fn:app::f -> method:app::<Dog as Speak>::speak",
            "fn:app::f -> method:app::Speak::speak",
        ]
    );
}

#[test]
fn trait_default_methods_dispatch_through_self() {
    let r = single(
        "pub trait Shape {\n    fn area(&self) -> u32;\n    fn describe(&self) -> u32 { self.area() + Self::unit() }\n    fn unit() -> u32 { 1 }\n}\n",
    );
    assert_eq!(
        calls(&r),
        vec![
            "method:app::Shape::describe -> method:app::Shape::area",
            "method:app::Shape::describe -> method:app::Shape::unit",
        ]
    );
}

#[test]
fn rehomes_impls_written_in_other_files() {
    let (files, r) = resolve(&[
        ("src/lib.rs", "app", "pub mod model;\npub mod ext;\n"),
        ("src/model.rs", "app::model", "pub struct Item;\n"),
        (
            "src/ext.rs",
            "app::ext",
            "use crate::model::Item;\nimpl Item { pub fn price(&self) -> u32 { helper() } }\nfn helper() -> u32 { 1 }\nfn f(i: Item) { i.price(); }\n",
        ),
    ]);
    let method = files
        .iter()
        .flat_map(|f| &f.symbols)
        .find(|s| s.name == "price")
        .unwrap();
    assert_eq!(method.id.as_str(), "method:app::model::Item::price");
    assert_eq!(method.file, "src/ext.rs");
    // The method still resolves names in the module its impl is written in.
    assert_eq!(
        calls(&r),
        vec![
            "fn:app::ext::f -> method:app::model::Item::price",
            "method:app::model::Item::price -> fn:app::ext::helper",
        ]
    );
}

#[test]
fn cyclic_glob_imports_terminate() {
    let r = single(
        "mod a { pub use crate::b::*; pub fn in_a() {} }\nmod b { pub use crate::a::*; }\nfn f() { b::in_a(); b::nowhere(); }\n",
    );
    assert_eq!(calls(&r), vec!["fn:app::f -> fn:app::a::in_a"]);
    assert_eq!(r.unresolved_calls.len(), 1);
}

#[test]
fn counts_import_outcomes() {
    let r = single(
        "use std::collections::HashMap;\nuse crate::a::{x, missing};\nmod a { pub fn x() {} }\n",
    );
    let imports = &r.stats.imports;
    assert_eq!(
        (imports.resolved, imports.external, imports.unresolved),
        (1, 1, 1)
    );
    assert!(r
        .edges
        .iter()
        .any(|e| e.kind == EdgeKind::Imports && e.to.as_str() == "fn:app::a::x"));
}

#[test]
fn calls_to_local_closures_are_not_repository_calls() {
    let r = single("fn m() {}\nfn f() {\n    let m = |x: u32| x;\n    m(1);\n}\nfn g(callback: impl Fn()) { callback(); }\n");
    assert!(calls(&r).is_empty());
    assert!(r.unresolved_calls.is_empty());
    assert_eq!(r.stats.calls.local, 2);
}

#[test]
fn private_imports_are_visible_through_self_and_child_globs() {
    let r = single(
        "mod a { pub struct Hit; impl Hit { pub fn new() -> Hit { Hit } } }\nmod b {\n    use crate::a::Hit;\n    fn f() { self::Hit::new(); }\n    mod tests {\n        use super::*;\n        fn t() { Hit::new(); }\n    }\n}\n",
    );
    assert_eq!(
        calls(&r),
        vec![
            "fn:app::b::f -> method:app::a::Hit::new",
            "fn:app::b::tests::t -> method:app::a::Hit::new",
        ]
    );
}

#[test]
fn follows_type_aliases_and_external_reexports() {
    let r = single(
        "mod store { pub struct Db; impl Db { pub fn open() -> Db { Db } } }\nmod fnv { pub type Map<K> = std::collections::HashMap<K, u8>; }\nmod facade { pub use std::mem::drop; pub type Store = crate::store::Db; }\nfn f() {\n    facade::Store::open();\n    fnv::Map::<u8>::default();\n    facade::drop(1);\n    ::regex::Regex::new(\"x\");\n}\n",
    );
    assert_eq!(calls(&r), vec!["fn:app::f -> method:app::store::Db::open"]);
    assert!(r.unresolved_calls.is_empty(), "{:?}", r.unresolved_calls);
    assert_eq!(r.stats.calls.external, 3);
}

#[test]
fn tuple_structs_reached_through_paths_are_constructors() {
    let r = single("mod sinks { pub struct Lossy(pub u8); }\nfn f() { sinks::Lossy(1); crate::sinks::Lossy(2); }\n");
    assert!(r.unresolved_calls.is_empty());
    assert_eq!(r.stats.calls.constructors, 2);
}

#[test]
fn follows_builder_chains_and_try_through_declared_return_types() {
    let r = single(
        r#"
pub struct Cmd;
pub struct Output;
impl Output { pub fn check(&self) {} }
impl Cmd {
    pub fn new() -> Cmd { Cmd }
    pub fn arg(&mut self, a: &str) -> &mut Cmd { self }
    pub fn run(&self) -> Result<Output, String> { Ok(Output) }
}
fn f() -> Result<(), String> {
    let mut cmd = Cmd::new();
    cmd.arg("a").arg("b").run()?.check();
    Ok(())
}
"#,
    );
    assert_eq!(
        calls(&r),
        vec![
            "fn:app::f -> method:app::Cmd::arg",
            "fn:app::f -> method:app::Cmd::new",
            "fn:app::f -> method:app::Cmd::run",
            "fn:app::f -> method:app::Output::check",
        ]
    );
    assert!(r.ambiguous_calls.is_empty());
}

#[test]
fn typed_closure_parameters_inside_macros_feed_inference() {
    let r = single(
        "pub struct TestCommand;\nimpl TestCommand { pub fn stdout(&self) -> String { String::new() } }\nmacro_rules! rgtest { ($name:ident, $f:expr) => {}; }\nrgtest!(case, |cmd: TestCommand| {\n    cmd.stdout();\n});\n",
    );
    assert_eq!(
        calls(&r),
        vec!["mod:app -> method:app::TestCommand::stdout"]
    );
}
