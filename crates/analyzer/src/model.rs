//! Domain types produced by static analysis.
//!
//! These types are deliberately independent of any storage backend: the store
//! crate maps them onto Neo4j, and the algorithms operate on them directly.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Deterministic symbol identifier: `<kind>:<qualified name>`.
///
/// IDs never contain line numbers, so they survive edits that move code
/// around. Collisions (e.g. two `#[cfg]`-gated definitions of the same
/// function) receive a deterministic `#N` suffix in source order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SymbolId(String);

impl SymbolId {
    pub fn new(kind: SymbolKind, qualified_name: &str) -> Self {
        Self(format!("{}:{}", kind.tag(), qualified_name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Wraps an ID read back from storage. IDs are opaque; only the
    /// analyzer creates new ones.
    pub fn from_stored(id: String) -> Self {
        Self(id)
    }

    pub(crate) fn with_suffix(&self, n: u32) -> Self {
        Self(format!("{}#{n}", self.0))
    }
}

impl fmt::Display for SymbolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Module,
    Struct,
    Enum,
    Trait,
    Function,
    Method,
}

impl SymbolKind {
    /// Lower-case name, as stored in the graph (`function`, `method`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Module => "module",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "module" => SymbolKind::Module,
            "struct" => SymbolKind::Struct,
            "enum" => SymbolKind::Enum,
            "trait" => SymbolKind::Trait,
            "function" => SymbolKind::Function,
            "method" => SymbolKind::Method,
            _ => return None,
        })
    }

    pub fn tag(self) -> &'static str {
        match self {
            SymbolKind::Module => "mod",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Function => "fn",
            SymbolKind::Method => "method",
        }
    }

    pub fn is_callable(self) -> bool {
        matches!(self, SymbolKind::Function | SymbolKind::Method)
    }

    pub fn is_type(self) -> bool {
        matches!(
            self,
            SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Trait
        )
    }
}

/// Visibility as written in source. Trait-impl methods carry no modifier and
/// are recorded as `Private` here; their effective visibility is the trait's.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "scope", content = "path")]
pub enum Visibility {
    Public,
    Crate,
    Super,
    Restricted(String),
    Private,
}

/// 1-based, inclusive line range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub id: SymbolId,
    pub kind: SymbolKind,
    pub name: String,
    pub qualified_name: String,
    /// Repository-relative path using `/` separators.
    pub file: String,
    pub span: Span,
    pub parent: Option<SymbolId>,
    pub visibility: Visibility,
    pub signature: Option<String>,
    /// Carries a test attribute such as `#[test]` or `#[tokio::test]`.
    pub is_test: bool,
    /// Declared inside a `#[cfg(test)]` item (test helpers, test modules).
    pub cfg_test: bool,
    /// Declared return type of functions and methods; used by the resolver
    /// to infer the type of `let x = Type::new()` bindings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_type: Option<TypeRef>,
    /// Generic type parameters in scope for a function or method,
    /// including those of its `impl` or `trait` block.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_params: Vec<String>,
}

impl Symbol {
    /// `module::Type::method` → `Type::method`; used for compact display.
    pub fn short_name(&self) -> String {
        let mut parts = self.qualified_name.rsplitn(3, "::");
        let last = parts.next().unwrap_or_default();
        match (self.kind, parts.next()) {
            (SymbolKind::Method, Some(owner)) => format!("{owner}::{last}"),
            _ => last.to_string(),
        }
    }
}

/// A type as written in source, reduced to what method resolution needs:
/// the path and its path-like generic arguments. References, `Box`, `Rc`,
/// `Arc`, `dyn Trait` and `impl Trait` are unwrapped to the inner type,
/// since method calls auto-deref through them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeRef {
    pub path: PathSegments,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<PathSegments>,
}

/// A path as written in source, with generic arguments stripped.
/// Leading `crate`, `self`, `super` and `Self` are kept as segments; a leading
/// `::` (global path) is represented by an empty first segment.
pub type PathSegments = Vec<String>;

/// One flattened item of a `use` declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    /// Module (or function, for block-scoped `use`) the import is visible in.
    pub scope: SymbolId,
    pub path: PathSegments,
    pub alias: Option<String>,
    pub glob: bool,
    pub visibility: Visibility,
    pub line: u32,
}

impl Import {
    /// Name this import binds in its scope (`None` for globs and `as _`).
    pub fn bound_name(&self) -> Option<&str> {
        if self.glob {
            return None;
        }
        match self.alias.as_deref() {
            Some("_") => None,
            Some(alias) => Some(alias),
            None => self.path.last().map(String::as_str),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Callee {
    /// `foo()`, `a::b::foo()`, `Type::method()`, `<T as Trait>::method()`.
    Path {
        segments: PathSegments,
        as_trait: Option<PathSegments>,
    },
    /// `receiver.method()`.
    Method { receiver: Receiver, name: String },
    /// Calls through closures, function pointers or other expressions; not
    /// statically resolvable without type inference.
    Dynamic { expression: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "text")]
pub enum Receiver {
    /// `self.method()`
    SelfValue,
    /// `self.field.method()`
    SelfField(String),
    /// `binding.method()` where `binding` is a local variable or parameter.
    Variable(String),
    /// `Type::constructor(..).method()`
    PathCall(PathSegments),
    /// `receiver.inner(..).method()`: the result of another method call.
    MethodCall {
        receiver: Box<Receiver>,
        method: String,
    },
    /// `expr?.method()`
    Try(Box<Receiver>),
    /// Any other receiver expression, truncated for display.
    Expr(String),
}

impl Receiver {
    pub fn display(&self) -> String {
        match self {
            Receiver::SelfValue => "self".into(),
            Receiver::SelfField(field) => format!("self.{field}"),
            Receiver::Variable(name) => name.clone(),
            Receiver::PathCall(path) => format!("{}(..)", path.join("::")),
            Receiver::MethodCall { receiver, method } => {
                format!("{}.{method}(..)", receiver.display())
            }
            Receiver::Try(inner) => format!("{}?", inner.display()),
            Receiver::Expr(text) => text.clone(),
        }
    }
}

impl Callee {
    /// Source-like rendering, e.g. `a::b`, `<T as Tr>::m`, `self.repo.save`.
    pub fn display(&self) -> String {
        match self {
            Callee::Path {
                segments,
                as_trait: Some(tr),
            } => match segments.split_last() {
                Some((name, ty)) => format!("<{} as {}>::{name}", ty.join("::"), tr.join("::")),
                None => String::new(),
            },
            Callee::Path { segments, .. } => segments.join("::"),
            Callee::Method { receiver, name } => format!("{}.{name}", receiver.display()),
            Callee::Dynamic { expression } => expression.clone(),
        }
    }
}

/// How a local name was bound, for receiver-type inference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BindingSource {
    /// `x: Type` (parameter) or `let x: Type = ..`
    Annotated { ty: TypeRef },
    /// `let x = path(..)` or `let x = path(..)?`
    CallResult {
        callee: PathSegments,
        unwrapped: bool,
    },
    /// `let x = Type { .. }`
    StructLiteral { ty: PathSegments },
    /// Any other binding (closures, untyped values, patterns). Recorded so
    /// that a call such as `f(x)` can be recognised as a call to a local.
    Untyped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalBinding {
    /// Function the binding belongs to.
    pub scope: SymbolId,
    pub name: String,
    pub source: BindingSource,
    pub line: u32,
}

/// `type Name = Target;`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeAlias {
    pub scope: SymbolId,
    pub name: String,
    pub target: TypeRef,
    pub line: u32,
}

/// A named field of a struct, for resolving `self.field.method()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDecl {
    pub owner: SymbolId,
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallSite {
    /// Innermost enclosing symbol (function, method, or module for calls in
    /// item initializers).
    pub caller: SymbolId,
    pub callee: Callee,
    pub line: u32,
    /// Found by re-parsing a macro invocation's arguments.
    pub in_macro: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImplBlock {
    /// Module (or enclosing function) the impl is written in.
    pub scope: SymbolId,
    /// Self type path, or the type's source text for non-path types (`[T]`, `&str`).
    pub self_type: PathSegments,
    pub trait_path: Option<PathSegments>,
    pub methods: Vec<SymbolId>,
    pub span: Span,
}

/// `mod name;` without a body; the module itself lives in another file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleDecl {
    pub parent: SymbolId,
    pub name: String,
    pub visibility: Visibility,
    pub line: u32,
    pub cfg_test: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAnalysis {
    pub path: String,
    pub crate_name: String,
    /// Symbol for the module this file defines.
    pub module: Option<SymbolId>,
    pub symbols: Vec<Symbol>,
    pub imports: Vec<Import>,
    pub calls: Vec<CallSite>,
    pub impls: Vec<ImplBlock>,
    pub module_decls: Vec<ModuleDecl>,
    pub bindings: Vec<LocalBinding>,
    pub fields: Vec<FieldDecl>,
    pub type_aliases: Vec<TypeAlias>,
    /// Number of ERROR / MISSING nodes tree-sitter produced for this file.
    pub syntax_errors: u32,
    pub loc: u32,
    /// FNV-1a hash of the file contents (hex), for change detection.
    pub content_hash: String,
}

impl FileAnalysis {
    /// Rewrites every symbol ID in this file according to `renames`.
    pub(crate) fn remap_ids(&mut self, renames: &HashMap<SymbolId, SymbolId>) {
        let remap = |id: &mut SymbolId| {
            if let Some(new) = renames.get(id) {
                *id = new.clone();
            }
        };
        self.module.iter_mut().for_each(remap);
        for symbol in &mut self.symbols {
            remap(&mut symbol.id);
            symbol.parent.iter_mut().for_each(remap);
        }
        self.imports.iter_mut().for_each(|i| remap(&mut i.scope));
        self.calls.iter_mut().for_each(|c| remap(&mut c.caller));
        self.module_decls
            .iter_mut()
            .for_each(|d| remap(&mut d.parent));
        self.bindings.iter_mut().for_each(|b| remap(&mut b.scope));
        self.fields.iter_mut().for_each(|f| remap(&mut f.owner));
        self.type_aliases
            .iter_mut()
            .for_each(|a| remap(&mut a.scope));
        for block in &mut self.impls {
            remap(&mut block.scope);
            block.methods.iter_mut().for_each(remap);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Lib,
    Bin,
    Test,
    Example,
    Bench,
    BuildScript,
}

/// A Cargo build target (a Rust crate in the compiler sense).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrateTarget {
    pub name: String,
    pub package: String,
    pub kind: TargetKind,
    pub root_file: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EdgeKind {
    Calls,
    Imports,
    Implements,
}

/// How the target of an edge was determined. Recorded on every edge so that
/// resolution quality can be measured and filtered per strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionMethod {
    /// Name defined in the enclosing scope (same module or function).
    Scope,
    /// Name bound by a `use` declaration.
    Import,
    /// Path anchored at `crate`, `self`, `super` or a crate name.
    Path,
    /// `Self::f()`, `self.f()` or `self.field.f()`.
    SelfType,
    /// Method call whose receiver type was inferred from a parameter,
    /// annotation, constructor call or struct literal.
    ReceiverType,
    /// `impl Trait for Type` and trait-method implementations.
    ImplBlock,
}

impl ResolutionMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            ResolutionMethod::Scope => "scope",
            ResolutionMethod::Import => "import",
            ResolutionMethod::Path => "path",
            ResolutionMethod::SelfType => "self_type",
            ResolutionMethod::ReceiverType => "receiver_type",
            ResolutionMethod::ImplBlock => "impl_block",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "scope" => ResolutionMethod::Scope,
            "import" => ResolutionMethod::Import,
            "path" => ResolutionMethod::Path,
            "self_type" => ResolutionMethod::SelfType,
            "receiver_type" => ResolutionMethod::ReceiverType,
            "impl_block" => ResolutionMethod::ImplBlock,
            _ => return None,
        })
    }
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeKind::Calls => "CALLS",
            EdgeKind::Imports => "IMPORTS",
            EdgeKind::Implements => "IMPLEMENTS",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "CALLS" => EdgeKind::Calls,
            "IMPORTS" => EdgeKind::Imports,
            "IMPLEMENTS" => EdgeKind::Implements,
            _ => return None,
        })
    }
}

/// A resolved, statically justified relationship between two symbols.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: SymbolId,
    pub to: SymbolId,
    pub kind: EdgeKind,
    pub via: ResolutionMethod,
    /// Source lines (in `from`'s file) that justify the edge.
    pub lines: Vec<u32>,
}
