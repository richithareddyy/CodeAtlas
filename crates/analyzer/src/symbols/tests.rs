use super::{extract_file, FileContext};
use crate::model::{CallSite, Callee, FileAnalysis, Symbol, SymbolKind, Visibility};
use crate::parser::RustParser;

fn extract(src: &str) -> FileAnalysis {
    let mut parser = RustParser::new().unwrap();
    let tree = parser.parse(src, "test.rs").unwrap();
    let module_path = vec!["app".to_string(), "m".to_string()];
    let ctx = FileContext {
        path: "src/m.rs",
        crate_name: "app",
        module_path: &module_path,
    };
    extract_file(&ctx, src, &tree, &mut parser)
}

fn symbol<'a>(fa: &'a FileAnalysis, id: &str) -> &'a Symbol {
    fa.symbols
        .iter()
        .find(|s| s.id.as_str() == id)
        .unwrap_or_else(|| {
            let ids: Vec<_> = fa.symbols.iter().map(|s| s.id.as_str()).collect();
            panic!("no symbol {id}; have {ids:?}")
        })
}

/// Renders calls as `caller -> callee @line` for compact assertions.
fn calls(fa: &FileAnalysis) -> Vec<String> {
    fa.calls.iter().map(render_call).collect()
}

fn render_call(c: &CallSite) -> String {
    let callee = match &c.callee {
        Callee::Dynamic { expression } => format!("dyn {expression}"),
        other => other.display(),
    };
    let caller = c.caller.as_str().rsplit("::").next().unwrap();
    let macro_flag = if c.in_macro { " [macro]" } else { "" };
    format!("{caller} -> {callee} @{}{macro_flag}", c.line)
}

#[test]
fn extracts_items_with_qualified_names_spans_and_visibility() {
    let fa = extract(
        "pub struct Order {\n    id: u64,\n}\n\npub(crate) enum State { A }\n\npub trait Store {}\n\nfn helper(x: u8) -> u8 {\n    x\n}\n",
    );
    let module = symbol(&fa, "mod:app::m");
    assert_eq!(module.kind, SymbolKind::Module);
    assert_eq!(fa.module.as_ref(), Some(&module.id));

    let order = symbol(&fa, "struct:app::m::Order");
    assert_eq!((order.span.start_line, order.span.end_line), (1, 3));
    assert_eq!(order.visibility, Visibility::Public);
    assert_eq!(order.signature.as_deref(), Some("pub struct Order"));
    assert_eq!(order.parent.as_ref(), Some(&module.id));

    assert_eq!(
        symbol(&fa, "enum:app::m::State").visibility,
        Visibility::Crate
    );
    assert_eq!(symbol(&fa, "trait:app::m::Store").kind, SymbolKind::Trait);

    let helper = symbol(&fa, "fn:app::m::helper");
    assert_eq!(helper.visibility, Visibility::Private);
    assert_eq!(helper.signature.as_deref(), Some("fn helper(x: u8) -> u8"));
    assert_eq!((helper.span.start_line, helper.span.end_line), (9, 11));
}

#[test]
fn names_inherent_and_trait_impl_methods_distinctly() {
    let fa = extract(
        "struct Money;\nimpl Money {\n    pub fn fmt(&self) {}\n}\nimpl std::fmt::Display for Money {\n    fn fmt(&self, f: &mut Formatter) -> Result { Ok(()) }\n}\nimpl<T> From<T> for Money { fn from(_: T) -> Self { Money } }\n",
    );
    let inherent = symbol(&fa, "method:app::m::Money::fmt");
    assert_eq!(inherent.kind, SymbolKind::Method);
    symbol(&fa, "method:app::m::<Money as std::fmt::Display>::fmt");
    symbol(&fa, "method:app::m::<Money as From<T>>::from");

    assert_eq!(fa.impls.len(), 3);
    assert_eq!(fa.impls[0].self_type, vec!["Money"]);
    assert_eq!(fa.impls[0].trait_path, None);
    assert_eq!(
        fa.impls[1].trait_path,
        Some(vec!["std".into(), "fmt".into(), "Display".into()])
    );
    assert_eq!(fa.impls[1].methods.len(), 1);
}

#[test]
fn extracts_trait_methods_with_and_without_default_bodies() {
    let fa = extract("pub trait Repo {\n    fn load(&self) -> u8;\n    fn reload(&self) -> u8 { self.load() }\n}\n");
    let load = symbol(&fa, "method:app::m::Repo::load");
    assert_eq!(load.parent.as_ref().unwrap().as_str(), "trait:app::m::Repo");
    assert_eq!(load.signature.as_deref(), Some("fn load(&self) -> u8"));
    symbol(&fa, "method:app::m::Repo::reload");
    assert_eq!(calls(&fa), vec!["reload -> self.load @3"]);
}

#[test]
fn detects_tests_and_cfg_test_scopes() {
    let fa = extract(
        "fn prod() {}\n#[cfg(test)]\nmod tests {\n    use super::*;\n    fn fixture() {}\n    #[test]\n    fn works() { prod(); }\n    #[tokio::test]\n    async fn async_works() {}\n}\n",
    );
    assert!(!symbol(&fa, "fn:app::m::prod").cfg_test);
    let tests_mod = symbol(&fa, "mod:app::m::tests");
    assert!(tests_mod.cfg_test);
    let fixture = symbol(&fa, "fn:app::m::tests::fixture");
    assert!(fixture.cfg_test && !fixture.is_test);
    let works = symbol(&fa, "fn:app::m::tests::works");
    assert!(works.cfg_test && works.is_test);
    assert!(symbol(&fa, "fn:app::m::tests::async_works").is_test);
    assert_eq!(fa.imports[0].scope.as_str(), "mod:app::m::tests");
}

#[test]
fn extracts_call_forms() {
    let fa = extract(
        r#"fn run(&self) {
    plain();
    crate::a::qualified();
    Self::assoc();
    Vec::<u8>::with_capacity(1);
    self.method();
    self.repo.save().unwrap();
    generic::<u8>();
    <Money as Display>::fmt();
    (make_closure())();
    outer(inner());
}
"#,
    );
    assert_eq!(
        calls(&fa),
        vec![
            "run -> plain @2",
            "run -> crate::a::qualified @3",
            "run -> Self::assoc @4",
            "run -> Vec::with_capacity @5",
            "run -> self.method @6",
            "run -> self.repo.save(..).unwrap @7",
            "run -> self.repo.save @7",
            "run -> generic @8",
            "run -> <Money as Display>::fmt @9",
            "run -> dyn (make_closure()) @10",
            "run -> make_closure @10",
            "run -> outer @11",
            "run -> inner @11",
        ]
    );
}

#[test]
fn finds_calls_inside_macro_arguments_with_correct_lines() {
    let fa = extract(
        "fn t() {\n    assert_eq!(\n        compute(1),\n        2\n    );\n    let v = vec![build(); 3];\n    println!(\"{}\", format!(\"{}\", render()));\n    my_dsl! { not_a_call() }\n}\n",
    );
    assert_eq!(
        calls(&fa),
        vec![
            "t -> compute @3 [macro]",
            "t -> build @6 [macro]",
            "t -> render @7 [macro]",
        ]
    );
}

#[test]
fn attributes_calls_to_the_innermost_symbol() {
    let fa = extract(
        "static CONFIG: Lazy<Config> = Lazy::new(load);\nfn outer() {\n    fn nested() { deep(); }\n    let f = |x| closure_call(x);\n    f(1);\n}\n",
    );
    symbol(&fa, "fn:app::m::outer::nested");
    assert_eq!(
        calls(&fa),
        vec![
            "m -> Lazy::new @1",
            "nested -> deep @3",
            "outer -> closure_call @4",
            "outer -> f @5",
        ]
    );
}

#[test]
fn suffixes_duplicate_definitions_deterministically() {
    let fa =
        extract("#[cfg(unix)]\nfn fee() -> u8 { 1 }\n#[cfg(not(unix))]\nfn fee() -> u8 { 2 }\n");
    assert_eq!(symbol(&fa, "fn:app::m::fee").span.start_line, 2);
    assert_eq!(symbol(&fa, "fn:app::m::fee#2").span.start_line, 4);
}

#[test]
fn records_inline_modules_and_module_declarations() {
    let fa = extract("pub mod api;\nmod inner {\n    pub fn f() {}\n    pub(super) mod deeper { fn g() {} }\n}\n");
    assert_eq!(fa.module_decls.len(), 1);
    assert_eq!(fa.module_decls[0].name, "api");
    assert_eq!(fa.module_decls[0].visibility, Visibility::Public);
    assert_eq!(fa.module_decls[0].parent.as_str(), "mod:app::m");

    symbol(&fa, "fn:app::m::inner::f");
    let deeper = symbol(&fa, "mod:app::m::inner::deeper");
    assert_eq!(deeper.visibility, Visibility::Super);
    assert_eq!(
        deeper.parent.as_ref().unwrap().as_str(),
        "mod:app::m::inner"
    );
    symbol(&fa, "fn:app::m::inner::deeper::g");
}

#[test]
fn scopes_block_level_imports_to_the_function() {
    let fa = extract("fn f() {\n    use crate::db::{connect as open};\n    open();\n}\n");
    assert_eq!(fa.imports.len(), 1);
    let import = &fa.imports[0];
    assert_eq!(import.scope.as_str(), "fn:app::m::f");
    assert_eq!(import.bound_name(), Some("open"));
    assert_eq!(import.line, 2);
}

#[test]
fn keeps_extracting_after_syntax_errors() {
    let fa = extract("fn good() { a(); }\nfn broken( {\nfn also_good() { b(); }\n");
    assert!(fa.syntax_errors > 0);
    symbol(&fa, "fn:app::m::good");
    assert!(calls(&fa).contains(&"good -> a @1".to_string()));
}

#[test]
fn fingerprints_ignore_formatting_and_comments_but_not_tokens() {
    let fp = |src: &str, id: &str| symbol(&extract(src), id).fingerprint.clone().unwrap();
    let base = fp(
        "fn total(a: u64) -> u64 {\n    a + 1\n}\n",
        "fn:app::m::total",
    );
    let reformatted = fp(
        "fn total(a: u64)\n    -> u64\n{\n    // add one\n    a /* inline */ + 1\n}\n",
        "fn:app::m::total",
    );
    assert_eq!(base, reformatted);
    // Moved to another line: same tokens, same fingerprint.
    let moved = fp(
        "\n\n\nfn total(a: u64) -> u64 { a + 1 }\n",
        "fn:app::m::total",
    );
    assert_eq!(base, moved);

    let edited = fp(
        "fn total(a: u64) -> u64 {\n    a + 2\n}\n",
        "fn:app::m::total",
    );
    assert_ne!(base, edited);
    // Token boundaries matter: `a+1` is the same, `a1` is not.
    let joined = fp(
        "fn total(a1: u64) -> u64 {\n    a1\n}\n",
        "fn:app::m::total",
    );
    assert_ne!(base, joined);
}

#[test]
fn trait_fingerprints_exclude_their_methods_and_modules_have_none() {
    let fa = extract("pub trait Store {\n    fn get(&self) -> u8;\n}\n");
    let edited = extract("pub trait Store {\n    fn get(&self) -> u16;\n}\n");
    assert_eq!(
        symbol(&fa, "trait:app::m::Store").fingerprint,
        symbol(&edited, "trait:app::m::Store").fingerprint
    );
    assert_ne!(
        symbol(&fa, "method:app::m::Store::get").fingerprint,
        symbol(&edited, "method:app::m::Store::get").fingerprint
    );
    let supertrait = extract("pub trait Store: Clone {\n    fn get(&self) -> u8;\n}\n");
    assert_ne!(
        symbol(&fa, "trait:app::m::Store").fingerprint,
        symbol(&supertrait, "trait:app::m::Store").fingerprint
    );
    assert_eq!(symbol(&fa, "mod:app::m").fingerprint, None);
}
