//! Domain types produced by static analysis.
//!
//! These types are deliberately independent of any storage backend: the store
//! crate maps them onto Neo4j, and the algorithms operate on them directly.

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
    SelfValue,
    /// Receiver expression text, truncated for display.
    Expr(String),
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
    /// Number of ERROR / MISSING nodes tree-sitter produced for this file.
    pub syntax_errors: u32,
    pub loc: u32,
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
