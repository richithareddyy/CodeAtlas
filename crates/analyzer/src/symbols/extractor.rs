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
use crate::ingest::fnv1a;
use crate::model::{
    BindingSource, CallSite, Callee, FieldDecl, FileAnalysis, ImplBlock, Import, LocalBinding,
    ModuleDecl, Receiver, Span, Symbol, SymbolId, SymbolKind, TypeAlias, TypeRef, Visibility,
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
            content_hash: format!("{:016x}", fnv1a(src.as_bytes())),
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
    /// Generic type parameters visible here (from enclosing impl / trait / fn).
    type_params: Vec<String>,
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
            return_type: None,
            type_params: Vec::new(),
        });
        self.out.module = Some(id.clone());

        let scope = Scope {
            module: id.clone(),
            prefix: qualified_name,
            owner: id,
            cfg_test: attrs.cfg_test,
            type_params: Vec::new(),
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
                if let Some(id) = self.type_item(node, scope, t, SymbolKind::Struct) {
                    self.struct_fields(node, &id, t);
                }
            }
            // Bindings are recorded inside macro bodies too: they are not
            // items, only inputs to receiver-type inference.
            "let_declaration" => {
                self.let_binding(node, scope, t);
                self.visit_children(node, scope, t);
            }
            "closure_expression" => {
                self.closure_bindings(node, scope, t);
                self.visit_children(node, scope, t);
            }
            "enum_item" if declares_items => {
                self.type_item(node, scope, t, SymbolKind::Enum);
            }
            "trait_item" if declares_items => self.trait_item(node, scope, t),
            "impl_item" if declares_items => self.impl_item(node, scope, t),
            "use_declaration" if declares_items => self.use_declaration(node, scope, t),
            "extern_crate_declaration" if declares_items => self.extern_crate(node, scope, t),
            "type_item" if declares_items => self.type_alias(node, scope, t),
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
        let mut type_params = scope.type_params.clone();
        type_params.extend(type_parameters(node, t.src));
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
            return_type: node
                .child_by_field_name("return_type")
                .and_then(|r| type_ref(r, t.src)),
            type_params: type_params.clone(),
        });

        if !t.in_macro() {
            self.parameter_bindings(node, &id, t);
        }
        if let Some(body) = node.child_by_field_name("body") {
            let inner = Scope {
                module: scope.module.clone(),
                prefix: qualified_name,
                owner: id.clone(),
                cfg_test,
                type_params,
            };
            self.visit_children(body, &inner, t);
        }
        Some(id)
    }

    /// Parameters with path types (`repo: &Repo`, `auth: Box<dyn Authorizer>`).
    fn parameter_bindings(&mut self, function: Node<'_>, id: &SymbolId, t: Text<'_>) {
        let Some(params) = function.child_by_field_name("parameters") else {
            return;
        };
        let mut cursor = params.walk();
        for param in params.named_children(&mut cursor) {
            let (Some(pattern), Some(ty)) = (
                param.child_by_field_name("pattern"),
                param.child_by_field_name("type"),
            ) else {
                continue;
            };
            if pattern.kind() != "identifier" {
                continue;
            }
            let source = type_ref(ty, t.src)
                .map_or(BindingSource::Untyped, |ty| BindingSource::Annotated { ty });
            self.out.bindings.push(LocalBinding {
                scope: id.clone(),
                name: text(pattern, t.src).to_string(),
                source,
                line: t.line(function),
            });
        }
    }

    /// Closure parameters: typed ones (`|cmd: TestCommand|`) feed receiver
    /// inference; untyped ones mark the name as a local.
    fn closure_bindings(&mut self, node: Node<'_>, scope: &Scope, t: Text<'_>) {
        let Some(params) = node.child_by_field_name("parameters") else {
            return;
        };
        let line = t.line(node);
        let mut cursor = params.walk();
        for param in params.named_children(&mut cursor) {
            let (name, source) = match param.kind() {
                "identifier" => (param, BindingSource::Untyped),
                "parameter" => {
                    let Some(pattern) = param
                        .child_by_field_name("pattern")
                        .filter(|p| p.kind() == "identifier")
                    else {
                        continue;
                    };
                    let source = param
                        .child_by_field_name("type")
                        .and_then(|ty| type_ref(ty, t.src))
                        .map_or(BindingSource::Untyped, |ty| BindingSource::Annotated { ty });
                    (pattern, source)
                }
                _ => continue,
            };
            self.out.bindings.push(LocalBinding {
                scope: scope.owner.clone(),
                name: text(name, t.src).to_string(),
                source,
                line,
            });
        }
    }

    /// `let x: T = ..`, `let x = T::new(..)?`, `let x = T { .. }`.
    fn let_binding(&mut self, node: Node<'_>, scope: &Scope, t: Text<'_>) {
        let Some(pattern) = node
            .child_by_field_name("pattern")
            .filter(|p| p.kind() == "identifier")
        else {
            return;
        };
        let source = if let Some(ty) = node.child_by_field_name("type") {
            type_ref(ty, t.src).map(|ty| BindingSource::Annotated { ty })
        } else {
            node.child_by_field_name("value")
                .and_then(|value| binding_from_value(value, t.src))
        };
        self.out.bindings.push(LocalBinding {
            scope: scope.owner.clone(),
            name: text(pattern, t.src).to_string(),
            source: source.unwrap_or(BindingSource::Untyped),
            line: t.line(node),
        });
    }

    fn struct_fields(&mut self, node: Node<'_>, owner: &SymbolId, t: Text<'_>) {
        let Some(body) = node
            .child_by_field_name("body")
            .filter(|b| b.kind() == "field_declaration_list")
        else {
            return;
        };
        let mut cursor = body.walk();
        for field in body.named_children(&mut cursor) {
            let (Some(name), Some(ty)) = (
                field.child_by_field_name("name"),
                field.child_by_field_name("type"),
            ) else {
                continue;
            };
            if let Some(ty) = type_ref(ty, t.src) {
                self.out.fields.push(FieldDecl {
                    owner: owner.clone(),
                    name: text(name, t.src).to_string(),
                    ty,
                });
            }
        }
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
            return_type: None,
            type_params: Vec::new(),
        });
        let inner = Scope {
            module: id.clone(),
            prefix: qualified_name,
            owner: id,
            cfg_test,
            type_params: Vec::new(),
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
            return_type: None,
            type_params: type_parameters(node, t.src),
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
        let mut type_params = scope.type_params.clone();
        type_params.extend(type_parameters(node, t.src));
        let inner = Scope {
            module: scope.module.clone(),
            prefix: trait_symbol.qualified_name.clone(),
            owner: id,
            cfg_test: trait_symbol.cfg_test,
            type_params,
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

        let mut impl_scope = scope.clone();
        impl_scope.type_params.extend(type_parameters(node, t.src));

        let mut methods = Vec::new();
        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            let children: Vec<Node<'_>> = body.named_children(&mut cursor).collect();
            for child in children {
                if child.kind() == "function_item" {
                    if let Some(id) = self.function(child, &impl_scope, t, FnOwner::Impl, &prefix) {
                        methods.push(id);
                    }
                } else {
                    self.visit(child, &impl_scope, t);
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

    fn type_alias(&mut self, node: Node<'_>, scope: &Scope, t: Text<'_>) {
        let (Some(name), Some(target)) = (
            node.child_by_field_name("name"),
            node.child_by_field_name("type")
                .and_then(|ty| type_ref(ty, t.src)),
        ) else {
            return;
        };
        self.out.type_aliases.push(TypeAlias {
            scope: scope.owner.clone(),
            name: text(name, t.src).to_string(),
            target,
            line: t.line(node),
        });
    }

    /// `extern crate name as alias;` binds a crate name like a `use`.
    fn extern_crate(&mut self, node: Node<'_>, scope: &Scope, t: Text<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        self.out.imports.push(Import {
            scope: scope.owner.clone(),
            path: vec![text(name, t.src).to_string()],
            alias: node
                .child_by_field_name("alias")
                .map(|a| text(a, t.src).to_string()),
            glob: false,
            visibility: visibility(node, t.src),
            line: t.line(node),
        });
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
    let receiver = receiver(value, src);
    Callee::Method {
        receiver,
        name: text(field, src).to_string(),
    }
}

fn receiver(value: Node<'_>, src: &str) -> Receiver {
    match value.kind() {
        "self" => return Receiver::SelfValue,
        "identifier" => return Receiver::Variable(text(value, src).to_string()),
        "field_expression" => {
            let base = value.child_by_field_name("value");
            let field = value.child_by_field_name("field");
            if let (Some(base), Some(field)) = (base, field) {
                if base.kind() == "self" && field.kind() == "field_identifier" {
                    return Receiver::SelfField(text(field, src).to_string());
                }
            }
        }
        "call_expression" => {
            if let Some(function) = value.child_by_field_name("function") {
                let function = match function.kind() {
                    "generic_function" => {
                        function.child_by_field_name("function").unwrap_or(function)
                    }
                    _ => function,
                };
                if function.kind() == "field_expression" {
                    let inner = function.child_by_field_name("value");
                    let method = function.child_by_field_name("field");
                    if let (Some(inner), Some(method)) = (inner, method) {
                        return Receiver::MethodCall {
                            receiver: Box::new(receiver(inner, src)),
                            method: text(method, src).to_string(),
                        };
                    }
                } else if let Some(path) = path_segments(function, src) {
                    return Receiver::PathCall(path);
                }
            }
        }
        "try_expression" => {
            if let Some(inner) = value.named_child(0) {
                return Receiver::Try(Box::new(receiver(inner, src)));
            }
        }
        "parenthesized_expression" => {
            if let Some(inner) = value.named_child(0) {
                return receiver(inner, src);
            }
        }
        _ => {}
    }
    Receiver::Expr(truncate(&normalize_ws(text(value, src))))
}

fn binding_from_value(value: Node<'_>, src: &str) -> Option<BindingSource> {
    let (value, unwrapped) = match value.kind() {
        "try_expression" => (value.named_child(0)?, true),
        _ => (value, false),
    };
    match value.kind() {
        "call_expression" => {
            let function = value.child_by_field_name("function")?;
            if function.kind() == "field_expression" {
                return None;
            }
            Some(BindingSource::CallResult {
                callee: path_segments(function, src)?,
                unwrapped,
            })
        }
        "struct_expression" if !unwrapped => Some(BindingSource::StructLiteral {
            ty: path_segments(value.child_by_field_name("name")?, src)?,
        }),
        // Standard collection / formatting macros have a known result type.
        "macro_invocation" if !unwrapped => {
            let name = text(value.child_by_field_name("macro")?, src);
            let ty = match name {
                "vec" => "Vec",
                "format" => "String",
                _ => return None,
            };
            Some(BindingSource::Annotated {
                ty: TypeRef {
                    path: vec![ty.to_string()],
                    args: vec![],
                },
            })
        }
        _ => None,
    }
}

/// Reduces a type node to a [`TypeRef`], looking through references,
/// smart pointers and `dyn` / `impl` trait types.
pub(crate) fn type_ref(node: Node<'_>, src: &str) -> Option<TypeRef> {
    match node.kind() {
        "reference_type" | "pointer_type" => type_ref(node.child_by_field_name("type")?, src),
        "dynamic_type" | "abstract_type" => type_ref(node.child_by_field_name("trait")?, src),
        "generic_type" => {
            let path = path_segments(node.child_by_field_name("type")?, src)?;
            let arg_nodes: Vec<Node<'_>> = node
                .child_by_field_name("type_arguments")
                .map(|args| {
                    let mut cursor = args.walk();
                    args.named_children(&mut cursor).collect()
                })
                .unwrap_or_default();
            let is_pointer = matches!(path.last().map(String::as_str), Some("Box" | "Rc" | "Arc"));
            if is_pointer {
                if let Some(inner) = arg_nodes.first() {
                    return type_ref(*inner, src);
                }
            }
            let args = arg_nodes
                .iter()
                .filter_map(|arg| type_ref(*arg, src).map(|t| t.path))
                .collect();
            Some(TypeRef { path, args })
        }
        "type_identifier" | "scoped_type_identifier" => Some(TypeRef {
            path: path_segments(node, src)?,
            args: vec![],
        }),
        "primitive_type" => Some(TypeRef {
            path: vec![text(node, src).to_string()],
            args: vec![],
        }),
        // Slices and arrays: only their element type could carry methods
        // of interest, and method calls on them dispatch to `[T]` / `[T; N]`.
        "array_type" => Some(TypeRef {
            path: vec!["[]".to_string()],
            args: vec![],
        }),
        _ => None,
    }
}

/// Names of the generic type parameters declared on an item.
fn type_parameters(item: Node<'_>, src: &str) -> Vec<String> {
    let Some(params) = item.child_by_field_name("type_parameters") else {
        return Vec::new();
    };
    let mut cursor = params.walk();
    params
        .named_children(&mut cursor)
        .filter(|p| matches!(p.kind(), "type_parameter" | "constrained_type_parameter"))
        .filter_map(|p| {
            p.child_by_field_name("name")
                .or_else(|| p.child_by_field_name("left"))
                .or_else(|| p.named_child(0))
        })
        .map(|name| text(name, src).to_string())
        .collect()
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
