//! Lightweight, local receiver-type inference for method calls.
//!
//! This is not type inference in the compiler sense. It recognises the
//! receiver forms that dominate service-style Rust code:
//!
//! * `self.method()` and `self.field.method()` (declared field types)
//! * parameters and `let` bindings with written types
//! * `let x = Type::constructor(..)` / `let x = f(..)?` using the callee's
//!   declared return type (`Self`, a named type, or the first argument of
//!   `Result` / `Option` after `?`)
//! * `let x = Type { .. }` and `Type::constructor(..).method()`
//!
//! Anything else is reported as unknown, never guessed.

use super::scope::{Ns, PathResult, Resolver};
use crate::model::{BindingSource, PathSegments, Receiver, SymbolId, SymbolKind, TypeRef};

/// Standard-library types commonly written without a path. A receiver of
/// one of these types cannot dispatch to a repository method (unless the
/// repository implements its own trait for it, which is not tracked).
const FOREIGN_TYPES: &[&str] = &[
    "String",
    "str",
    "Vec",
    "VecDeque",
    "HashMap",
    "HashSet",
    "BTreeMap",
    "BTreeSet",
    "Option",
    "Result",
    "Box",
    "Rc",
    "Arc",
    "Cell",
    "RefCell",
    "Mutex",
    "RwLock",
    "PathBuf",
    "Path",
    "Duration",
    "Instant",
    "SystemTime",
];

/// Associated functions that, when produced by `#[derive]`, return `Self`.
const DERIVED_SELF_CONSTRUCTORS: &[&str] = &["default", "clone"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Inferred {
    /// A struct, enum or trait defined in the repository.
    Repo(SymbolId),
    /// A type known to be defined outside the repository.
    Foreign,
    Unknown,
}

pub(crate) fn is_foreign_path(path: &[String]) -> bool {
    match path {
        [] => false,
        [single] => FOREIGN_TYPES.contains(&single.as_str()),
        [first, ..] => matches!(first.as_str(), "std" | "core" | "alloc"),
    }
}

impl Resolver<'_> {
    /// True when a path rooted at `name` cannot lead into the repository:
    /// a standard-library root, a name bound by a `use` of an external
    /// crate, or a name that no repository module, type or crate has.
    pub fn is_external_root(&self, scope: &SymbolId, name: &str) -> bool {
        if matches!(name, "std" | "core" | "alloc")
            || FOREIGN_TYPES.contains(&name)
            || self.table.is_dependency(name)
        {
            return true;
        }
        if self.bound_to_external_import(scope, name) {
            return true;
        }
        !matches!(name, "crate" | "self" | "super" | "Self" | "")
            && !self.table.is_type_name(name)
            && !self.table.is_type_param(scope, name)
    }

    /// Whether `name` is bound, in `scope`'s lexical scope chain, by an
    /// explicit `use` whose target lies outside the repository.
    pub fn bound_to_external_import(&self, scope: &SymbolId, name: &str) -> bool {
        let mut current = Some(scope);
        while let Some(s) = current {
            for import in self.table.imports_in(s) {
                if import.bound_name() == Some(name) {
                    return matches!(
                        self.resolve_import(import),
                        super::outcome::ImportOutcome::External
                    );
                }
            }
            if self.table.kind(s) == Some(SymbolKind::Module) {
                break;
            }
            current = self.table.lexical_parent(s);
        }
        false
    }

    pub fn receiver_type(&self, caller: &SymbolId, receiver: &Receiver, line: u32) -> Inferred {
        match receiver {
            Receiver::SelfValue => self.self_inferred(caller),
            Receiver::SelfField(field) => {
                let Some(owner) = self.table.self_type(caller) else {
                    return Inferred::Unknown;
                };
                match self.table.field_type(owner, field) {
                    Some(ty) => self.type_ref_type(owner, ty),
                    None => Inferred::Unknown,
                }
            }
            Receiver::Variable(name) => self.binding_type(caller, name, line),
            Receiver::PathCall(path) => self.call_result_type(caller, path, false),
            Receiver::MethodCall { receiver, method } => {
                self.method_result_type(caller, receiver, method, line, false)
            }
            Receiver::Try(inner) => match inner.as_ref() {
                Receiver::PathCall(path) => self.call_result_type(caller, path, true),
                Receiver::MethodCall { receiver, method } => {
                    self.method_result_type(caller, receiver, method, line, true)
                }
                _ => Inferred::Unknown,
            },
            Receiver::Expr(_) => Inferred::Unknown,
        }
    }

    /// Whether the receiver's declared type is a generic parameter.
    pub fn receiver_is_generic(&self, caller: &SymbolId, receiver: &Receiver, line: u32) -> bool {
        let generic = |scope: &SymbolId, ty: &TypeRef| matches!(ty.path.as_slice(), [name] if self.table.is_type_param(scope, name));
        match receiver {
            Receiver::Variable(name) => self
                .table
                .bindings_in(caller)
                .iter()
                .filter(|b| &b.name == name && b.line <= line)
                .max_by_key(|b| b.line)
                .is_some_and(|b| match &b.source {
                    BindingSource::Annotated { ty } => generic(caller, ty),
                    _ => false,
                }),
            Receiver::SelfField(field) => self.table.self_type(caller).is_some_and(|owner| {
                self.table
                    .field_type(owner, field)
                    .is_some_and(|ty| generic(owner, ty))
            }),
            _ => false,
        }
    }

    fn self_inferred(&self, scope: &SymbolId) -> Inferred {
        self.table
            .self_type(scope)
            .map_or(Inferred::Unknown, |ty| Inferred::Repo(ty.clone()))
    }

    /// Uses the latest binding of `name` in `caller` that precedes `line`.
    fn binding_type(&self, caller: &SymbolId, name: &str, line: u32) -> Inferred {
        let binding = self
            .table
            .bindings_in(caller)
            .iter()
            .filter(|b| b.name == name && b.line <= line)
            .max_by_key(|b| b.line);
        let Some(binding) = binding else {
            return Inferred::Unknown;
        };
        match &binding.source {
            BindingSource::Annotated { ty } => self.type_ref_type(caller, ty),
            BindingSource::StructLiteral { ty } => self.type_path_type(caller, ty),
            BindingSource::CallResult { callee, unwrapped } => {
                self.call_result_type(caller, callee, *unwrapped)
            }
            BindingSource::Untyped => Inferred::Unknown,
        }
    }

    pub fn type_ref_type(&self, scope: &SymbolId, ty: &TypeRef) -> Inferred {
        self.type_path_type(scope, &ty.path)
    }

    fn type_path_type(&self, scope: &SymbolId, path: &PathSegments) -> Inferred {
        if path.len() == 1 && path[0] == "Self" {
            return self.self_inferred(scope);
        }
        match self.resolve_path(scope, path, Ns::Type, 0) {
            PathResult::Found(found) => match found.targets.as_slice() {
                [only] if self.table.kind(only).is_some_and(SymbolKind::is_type) => {
                    Inferred::Repo(only.clone())
                }
                _ => Inferred::Unknown,
            },
            _ if path.len() == 1 && self.table.is_type_param(scope, &path[0]) => Inferred::Unknown,
            _ if is_foreign_path(path) => Inferred::Foreign,
            _ if path
                .first()
                .is_some_and(|root| self.is_external_root(scope, root)) =>
            {
                Inferred::Foreign
            }
            _ => Inferred::Unknown,
        }
    }

    /// Type of `receiver.method(..)` from the method's declared return type.
    /// Only repository receivers are followed: the return types of standard
    /// library methods are not known.
    fn method_result_type(
        &self,
        caller: &SymbolId,
        receiver: &Receiver,
        method: &str,
        line: u32,
        unwrapped: bool,
    ) -> Inferred {
        let Inferred::Repo(ty) = self.receiver_type(caller, receiver, line) else {
            return Inferred::Unknown;
        };
        match self.associated(&ty, method).as_slice() {
            [target] => self.returned_type(target, unwrapped),
            _ => Inferred::Unknown,
        }
    }

    /// Declared return type of `callee`, unwrapping `Result` / `Option` when
    /// the call is followed by `?`.
    fn returned_type(&self, callee: &SymbolId, unwrapped: bool) -> Inferred {
        let Some(returns) = self
            .table
            .symbol(callee)
            .and_then(|s| s.return_type.as_ref())
        else {
            return Inferred::Unknown;
        };
        let returns = if unwrapped {
            let wrapper = returns.path.last().map(String::as_str);
            match (wrapper, returns.args.first()) {
                (Some("Result" | "Option"), Some(inner)) => TypeRef {
                    path: inner.clone(),
                    args: vec![],
                },
                _ => return Inferred::Unknown,
            }
        } else {
            returns.clone()
        };
        // Types in a signature are resolved in the callee's scope.
        self.type_ref_type(callee, &returns)
    }

    /// Type of `callee(..)` (or `callee(..)?` when `unwrapped`), from the
    /// callee's declared return type.
    fn call_result_type(
        &self,
        scope: &SymbolId,
        callee: &PathSegments,
        unwrapped: bool,
    ) -> Inferred {
        match self.resolve_path(scope, callee, Ns::Value, 0) {
            PathResult::Found(found) => {
                let [target] = found.targets.as_slice() else {
                    return Inferred::Unknown;
                };
                self.returned_type(target, unwrapped)
            }
            PathResult::AssocMissing { owner } if !unwrapped => {
                let derived = callee
                    .last()
                    .is_some_and(|name| DERIVED_SELF_CONSTRUCTORS.contains(&name.as_str()));
                if derived {
                    Inferred::Repo(owner)
                } else {
                    Inferred::Unknown
                }
            }
            _ if callee.len() > 1
                && (is_foreign_path(&callee[..callee.len() - 1])
                    || self.is_external_root(scope, &callee[0])) =>
            {
                Inferred::Foreign
            }
            _ => Inferred::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<String> {
        s.split("::").map(String::from).collect()
    }

    #[test]
    fn classifies_foreign_paths() {
        assert!(is_foreign_path(&p("HashMap")));
        assert!(is_foreign_path(&p("std::sync::Mutex")));
        assert!(!is_foreign_path(&p("T")));
        assert!(!is_foreign_path(&p("crate::model::Invoice")));
    }
}
