//! AST traversal that turns a parsed Rust file into a [`FileAnalysis`].
//!
//! The extractor records what is *written*: declarations, `use` items, impl
//! blocks and call sites with their textual callee paths. Deciding what a
//! callee path refers to is the resolver's job.

use std::collections::HashMap;

use tree_sitter::{Node, Tree};

use super::syntax::{
    end_line, normalize_ws, path_segments, signature, start_line, text, type_segments, visibility,
    ItemAttributes,
};
use super::use_tree::flatten_use;
use crate::ingest::discovery::count_loc;
use crate::model::{
    CallSite, Callee, FileAnalysis, ImplBlock, Import, ModuleDecl, Receiver, Span, Symbol,
    SymbolId, SymbolKind, Visibility,
};
use crate::parser::{count_syntax_errors, RustParser};

/// Nested macro invocations deeper than this are not re-parsed.
const MAX_MACRO_DEPTH: u8 = 4;
/// Receiver / dynamic-callee text kept for display.
const MAX_EXPR_CHARS: usize = 60;

pub struct FileContext<'a> {
    pub path: &'a str,
    pub crate_name: &'a str,
    /// Module path of the file, starting with the crate name.
    pub module_path: &'a [String],
}

pub fn extract_file(
    ctx: &FileContext<'_>,
    src: &str,
    tree: &Tree,
    parser: &mut RustParser,
) -> FileAnalysis {
    let mut extractor = Extractor {
        file: ctx.path,
        parser,
        seen_ids: HashMap::new(),
        out: FileAnalysis {
            path: ctx.path.to_string(),
            crate_name: ctx.crate_name.to_string(),
            loc: count_loc(src),
            syntax_errors: count_syntax_errors(tree.root_node()),
            ..Default::default()
        },
    };
    extractor.file_module(ctx, tree.root_node(), src);
    extractor.out
}

struct Extractor<'a> {
    file: &'a str,
    parser: &'a mut RustParser,
    seen_ids: HashMap<SymbolId, u32>,
    out: FileAnalysis,
}

/// Where in the item hierarchy the walk currently is.
#[derive(Clone)]
struct Scope {
    /// Nearest enclosing module.
    module: SymbolId,
    /// Qualified-name prefix for items declared here.
    prefix: String,
    /// Symbol that owns calls and imports found here.
    owner: SymbolId,
    cfg_test: bool,
}

/// The text being walked: the file itself or a re-parsed macro body.
#[derive(Clone, Copy)]
struct Text<'s> {
    src: &'s str,
    /// Added to tree-sitter rows to obtain file line numbers.
    line_offset: u32,
    macro_depth: u8,
}

impl Text<'_> {
    fn line(&self, node: Node<'_>) -> u32 {
        start_line(node) + self.line_offset
    }

    fn in_macro(&self) -> bool {
        self.macro_depth > 0
    }
}

/// What kind of item a function belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FnOwner {
    Free,
    Impl,
    Trait,
}

impl Extractor<'_> {
    fn file_module(&mut self, ctx: &FileContext<'_>, root: Node<'_>, src: &str) {
        let qualified_name = ctx.module_path.join("::");
        let attrs = ItemAttributes::from_inner(root, src);
        let is_crate_root = ctx.module_path.len() == 1;
        let id = self.add_symbol(Symbol {
            id: SymbolId::new(SymbolKind::Module, &qualified_name),
            kind: SymbolKind::Module,
            name: ctx.module_path.last().cloned().unwrap_or_default(),
            qualified_name: qualified_name.clone(),
            file: self.file.to_string(),
            span: Span {
                start_line: 1,
                end_line: end_line(root).max(1),
            },
            parent: None,
            // File modules get their real visibility from the parent's
            // `mod` declaration during assembly.
            visibility: if is_crate_root {
                Visibility::Public
            } else {
                Visibility::Private
            },
            signature: None,
            is_test: false,
            cfg_test: attrs.cfg_test,
        });
        self.out.module = Some(id.clone());

        let scope = Scope {
            module: id.clone(),
            prefix: qualified_name,
            owner: id,
            cfg_test: attrs.cfg_test,
        };
        let text = Text {
            src,
            line_offset: 0,
            macro_depth: 0,
        };
        self.visit_children(root, &scope, text);
    }

    fn visit_children(&mut self, node: Node<'_>, scope: &Scope, t: Text<'_>) {
        let mut cursor = node.walk();
        let children: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
        for child in children {
            self.visit(child, scope, t);
        }
    }

    fn visit(&mut self, node: Node<'_>, scope: &Scope, t: Text<'_>) {
        // Items inside re-parsed macro bodies are not real declarations.
        let declares_items = !t.in_macro();
        match node.kind() {
            "function_item" if declares_items => {
                self.function(node, scope, t, FnOwner::Free, &scope.prefix);
            }
            "mod_item" if declares_items => self.module(node, scope, t),
            "struct_item" | "union_item" if declares_items => {
                self.type_item(node, scope, t, SymbolKind::Struct);
            }
            "enum_item" if declares_items => {
                self.type_item(node, scope, t, SymbolKind::Enum);
            }
            "trait_item" if declares_items => self.trait_item(node, scope, t),
            "impl_item" if declares_items => self.impl_item(node, scope, t),
            "use_declaration" if declares_items => self.use_declaration(node, scope, t),
            "call_expression" => {
                self.call(node, scope, t);
                self.visit_children(node, scope, t);
            }
            "macro_invocation" => self.macro_invocation(node, scope, t),
            "macro_definition"
            | "attribute_item"
            | "inner_attribute_item"
            | "line_comment"
            | "block_comment"
            | "string_literal"
            | "raw_string_literal" => {}
            _ => self.visit_children(node, scope, t),
        }
    }

    fn function(
        &mut self,
        node: Node<'_>,
        scope: &Scope,
        t: Text<'_>,
        owner: FnOwner,
        prefix: &str,
    ) -> Option<SymbolId> {
        let name = text(node.child_by_field_name("name")?, t.src).to_string();
        let attrs = ItemAttributes::from_preceding(node, t.src);
        let kind = match owner {
            FnOwner::Free => SymbolKind::Function,
            FnOwner::Impl | FnOwner::Trait => SymbolKind::Method,
        };
        let qualified_name = format!("{prefix}::{name}");
        let cfg_test = scope.cfg_test || attrs.cfg_test;
        let id = self.add_symbol(Symbol {
            id: SymbolId::new(kind, &qualified_name),
            kind,
            name,
            qualified_name: qualified_name.clone(),
            file: self.file.to_string(),
            span: span(node),
            parent: Some(scope.owner.clone()),
            visibility: visibility(node, t.src),
            signature: Some(signature(node, t.src)),
            is_test: attrs.is_test,
            cfg_test,
        });

        if let Some(body) = node.child_by_field_name("body") {
            let inner = Scope {
                module: scope.module.clone(),
                prefix: qualified_name,
                owner: id.clone(),
                cfg_test,
            };
            self.visit_children(body, &inner, t);
        }
        Some(id)
    }

    fn module(&mut self, node: Node<'_>, scope: &Scope, t: Text<'_>) {
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        let name = text(name_node, t.src).to_string();
        let attrs = ItemAttributes::from_preceding(node, t.src);
        let vis = visibility(node, t.src);

        let Some(body) = node.child_by_field_name("body") else {
            self.out.module_decls.push(ModuleDecl {
                parent: scope.module.clone(),
                name,
                visibility: vis,
                line: t.line(node),
                cfg_test: scope.cfg_test || attrs.cfg_test,
            });
            return;
        };

        let inner_attrs = ItemAttributes::from_inner(body, t.src);
        let cfg_test = scope.cfg_test || attrs.cfg_test || inner_attrs.cfg_test;
        let qualified_name = format!("{}::{name}", scope.prefix);
        let id = self.add_symbol(Symbol {
            id: SymbolId::new(SymbolKind::Module, &qualified_name),
            kind: SymbolKind::Module,
            name,
            qualified_name: qualified_name.clone(),
            file: self.file.to_string(),
            span: span(node),
            parent: Some(scope.owner.clone()),
            visibility: vis,
            signature: None,
            is_test: false,
            cfg_test,
        });
        let inner = Scope {
            module: id.clone(),
            prefix: qualified_name,
            owner: id,
            cfg_test,
        };
        self.visit_children(body, &inner, t);
    }

    fn type_item(
        &mut self,
        node: Node<'_>,
        scope: &Scope,
        t: Text<'_>,
        kind: SymbolKind,
    ) -> Option<SymbolId> {
        let name = text(node.child_by_field_name("name")?, t.src).to_string();
        let attrs = ItemAttributes::from_preceding(node, t.src);
        let qualified_name = format!("{}::{name}", scope.prefix);
        Some(self.add_symbol(Symbol {
            id: SymbolId::new(kind, &qualified_name),
            kind,
            name,
            qualified_name,
            file: self.file.to_string(),
            span: span(node),
            parent: Some(scope.owner.clone()),
            visibility: visibility(node, t.src),
            signature: Some(signature(node, t.src)),
            is_test: false,
            cfg_test: scope.cfg_test || attrs.cfg_test,
        }))
    }

    fn trait_item(&mut self, node: Node<'_>, scope: &Scope, t: Text<'_>) {
        let Some(id) = self.type_item(node, scope, t, SymbolKind::Trait) else {
            return;
        };
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        let trait_symbol = &self.out.symbols[self.out.symbols.len() - 1];
        let inner = Scope {
            module: scope.module.clone(),
            prefix: trait_symbol.qualified_name.clone(),
            owner: id,
            cfg_test: trait_symbol.cfg_test,
        };
        let mut cursor = body.walk();
        let children: Vec<Node<'_>> = body.named_children(&mut cursor).collect();
        for child in children {
            match child.kind() {
                "function_item" | "function_signature_item" => {
                    let prefix = inner.prefix.clone();
                    self.function(child, &inner, t, FnOwner::Trait, &prefix);
                }
                _ => self.visit(child, &inner, t),
            }
        }
    }

    fn impl_item(&mut self, node: Node<'_>, scope: &Scope, t: Text<'_>) {
        let Some(type_node) = node.child_by_field_name("type") else {
            return;
        };
        let self_type = type_segments(type_node, t.src);
        let trait_node = node.child_by_field_name("trait");
        let trait_path = trait_node.map(|n| type_segments(n, t.src));
        let type_name = self_type.last().cloned().unwrap_or_default();

        // Trait-impl methods use Rust's qualified-path form so that `fmt`
        // from `Display` and `fmt` from `Debug` get distinct names.
        let prefix = match trait_node {
            Some(tr) => format!(
                "{}::<{type_name} as {}>",
                scope.prefix,
                normalize_ws(text(tr, t.src))
            ),
            None => format!("{}::{type_name}", scope.prefix),
        };

        let mut methods = Vec::new();
        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            let children: Vec<Node<'_>> = body.named_children(&mut cursor).collect();
            for child in children {
                if child.kind() == "function_item" {
                    if let Some(id) = self.function(child, scope, t, FnOwner::Impl, &prefix) {
                        methods.push(id);
                    }
                } else {
                    self.visit(child, scope, t);
                }
            }
        }

        self.out.impls.push(ImplBlock {
            scope: scope.owner.clone(),
            self_type,
            trait_path,
            methods,
            span: span(node),
        });
    }

    fn use_declaration(&mut self, node: Node<'_>, scope: &Scope, t: Text<'_>) {
        let vis = visibility(node, t.src);
        let line = t.line(node);
        for item in flatten_use(node, t.src) {
            self.out.imports.push(Import {
                scope: scope.owner.clone(),
                path: item.path,
                alias: item.alias,
                glob: item.glob,
                visibility: vis.clone(),
                line,
            });
        }
    }

    fn call(&mut self, node: Node<'_>, scope: &Scope, t: Text<'_>) {
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        self.out.calls.push(CallSite {
            caller: scope.owner.clone(),
            callee: callee(function, t.src),
            line: t.line(node),
            in_macro: t.in_macro(),
        });
    }

    /// Tree-sitter leaves macro arguments as unparsed token trees. Arguments
    /// of `(..)` / `[..]` macros are re-parsed as an array expression so that
    /// calls inside `assert_eq!(f(x), 1)` or `vec![g(); n]` are not lost.
    fn macro_invocation(&mut self, node: Node<'_>, scope: &Scope, t: Text<'_>) {
        if t.macro_depth >= MAX_MACRO_DEPTH {
            return;
        }
        let mut cursor = node.walk();
        let Some(tokens) = node
            .named_children(&mut cursor)
            .filter(|c| c.kind() == "token_tree")
            .last()
        else {
            return;
        };
        let raw = text(tokens, t.src);
        if !(raw.starts_with('(') || raw.starts_with('[')) || raw.len() < 2 {
            return;
        }
        // The wrapper prefix contains no newline, so row N of the wrapped
        // text is row N of the token tree.
        let wrapped = format!("fn __codeatlas_macro() {{[{}];}}", &raw[1..raw.len() - 1]);
        let Ok(tree) = self.parser.parse(&wrapped, self.file) else {
            return;
        };
        let Some(body) = tree
            .root_node()
            .named_child(0)
            .and_then(|f| f.child_by_field_name("body"))
        else {
            return;
        };
        let inner = Text {
            src: &wrapped,
            line_offset: t.line_offset + tokens.start_position().row as u32,
            macro_depth: t.macro_depth + 1,
        };
        self.visit_children(body, scope, inner);
    }

    /// Registers a symbol, giving duplicate IDs a deterministic `#N` suffix.
    fn add_symbol(&mut self, mut symbol: Symbol) -> SymbolId {
        let base = symbol.id.clone();
        let count = self.seen_ids.entry(base.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            symbol.id = base.with_suffix(*count);
        }
        let id = symbol.id.clone();
        self.out.symbols.push(symbol);
        id
    }
}

fn callee(function: Node<'_>, src: &str) -> Callee {
    match function.kind() {
        "field_expression" => method_callee(function, src),
        "generic_function" => match function.child_by_field_name("function") {
            Some(inner) if inner.kind() == "field_expression" => method_callee(inner, src),
            Some(inner) => callee(inner, src),
            None => dynamic(function, src),
        },
        "scoped_identifier" => {
            let qualified = function
                .child_by_field_name("path")
                .filter(|p| p.kind() == "bracketed_type")
                .and_then(|p| p.named_child(0))
                .filter(|q| q.kind() == "qualified_type");
            match qualified {
                Some(q) => qualified_callee(function, q, src),
                None => path_callee(function, src),
            }
        }
        _ => path_callee(function, src),
    }
}

fn path_callee(node: Node<'_>, src: &str) -> Callee {
    match path_segments(node, src) {
        Some(segments) => Callee::Path {
            segments,
            as_trait: None,
        },
        None => dynamic(node, src),
    }
}

/// `<Type as Trait>::name(..)`
fn qualified_callee(function: Node<'_>, qualified: Node<'_>, src: &str) -> Callee {
    let (Some(ty), Some(name)) = (
        qualified.child_by_field_name("type"),
        function.child_by_field_name("name"),
    ) else {
        return dynamic(function, src);
    };
    let mut segments = type_segments(ty, src);
    segments.push(text(name, src).to_string());
    Callee::Path {
        segments,
        as_trait: qualified
            .child_by_field_name("alias")
            .map(|alias| type_segments(alias, src)),
    }
}

fn method_callee(field_expr: Node<'_>, src: &str) -> Callee {
    let (Some(value), Some(field)) = (
        field_expr.child_by_field_name("value"),
        field_expr.child_by_field_name("field"),
    ) else {
        return dynamic(field_expr, src);
    };
    let receiver = if value.kind() == "self" {
        Receiver::SelfValue
    } else {
        Receiver::Expr(truncate(&normalize_ws(text(value, src))))
    };
    Callee::Method {
        receiver,
        name: text(field, src).to_string(),
    }
}

fn dynamic(node: Node<'_>, src: &str) -> Callee {
    Callee::Dynamic {
        expression: truncate(&normalize_ws(text(node, src))),
    }
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_EXPR_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(MAX_EXPR_CHARS - 1).collect();
    out.push('…');
    out
}

fn span(node: Node<'_>) -> Span {
    Span {
        start_line: start_line(node),
        end_line: end_line(node),
    }
}
