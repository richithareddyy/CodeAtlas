//! Classification of call sites and imports into resolution outcomes.

use serde::{Deserialize, Serialize};

use super::infer::Inferred;
use super::scope::{Ns, PathResult, Resolver};
use crate::model::{CallSite, Callee, Import, Receiver, ResolutionMethod, SymbolId, SymbolKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbiguityReason {
    /// The path names several definitions (e.g. `#[cfg]` variants, or
    /// several trait impls providing the same method).
    MultipleDefinitions,
    /// Method call whose receiver type could not be inferred; the
    /// candidates are every repository method with that name.
    ReceiverTypeUnknown,
    /// As `ReceiverTypeUnknown`, but every candidate implements a trait from
    /// outside the repository (`Iterator::next`, `Clone::clone`, ...), so
    /// the call most likely targets a standard-library type.
    ForeignTraitMethod,
    /// `T::f()` or `x.f()` where `T` / `x` has a generic parameter type;
    /// any implementation of the bound may be called.
    GenericParameter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnresolvedReason {
    /// A repository function has this name, but it is not in scope at the
    /// call site (cfg-disabled code, macro-generated imports, ...).
    NameNotInScope,
    /// A path prefix resolved inside the repository but a later segment
    /// did not.
    PathSegmentNotFound,
    /// `Self` used where the self type is not a repository type.
    SelfTypeUnknown,
    /// Call through a closure, function pointer or other expression.
    DynamicCall,
}

impl AmbiguityReason {
    pub fn as_str(self) -> &'static str {
        match self {
            AmbiguityReason::MultipleDefinitions => "multiple_definitions",
            AmbiguityReason::ReceiverTypeUnknown => "receiver_type_unknown",
            AmbiguityReason::ForeignTraitMethod => "foreign_trait_method",
            AmbiguityReason::GenericParameter => "generic_parameter",
        }
    }
}

impl UnresolvedReason {
    pub fn as_str(self) -> &'static str {
        match self {
            UnresolvedReason::NameNotInScope => "name_not_in_scope",
            UnresolvedReason::PathSegmentNotFound => "path_segment_not_found",
            UnresolvedReason::SelfTypeUnknown => "self_type_unknown",
            UnresolvedReason::DynamicCall => "dynamic_call",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CallOutcome {
    Resolved {
        target: SymbolId,
        via: ResolutionMethod,
    },
    Ambiguous {
        candidates: Vec<SymbolId>,
        reason: AmbiguityReason,
    },
    Unresolved(UnresolvedReason),
    /// The target is not defined in the analysed repository: standard
    /// library, dependencies, derived methods, or local closures.
    External,
    /// Tuple-struct or enum-variant construction, not a function call.
    Constructor,
    /// Call of a closure or function value bound locally (`let f = |x| ..;
    /// f(1)`). The closure body's calls are attributed to the caller.
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImportOutcome {
    Resolved(Vec<SymbolId>),
    External,
    Unresolved,
}

impl Resolver<'_> {
    pub fn resolve_call(&self, call: &CallSite) -> CallOutcome {
        match &call.callee {
            Callee::Path {
                segments,
                as_trait: None,
            } => self.resolve_path_call(&call.caller, segments, call.line),
            Callee::Path {
                segments,
                as_trait: Some(trait_path),
            } => self.resolve_qualified_call(&call.caller, segments, trait_path),
            Callee::Method { receiver, name } => {
                self.resolve_method_call(&call.caller, receiver, name, call.line)
            }
            Callee::Dynamic { .. } => CallOutcome::Unresolved(UnresolvedReason::DynamicCall),
        }
    }

    fn resolve_path_call(&self, caller: &SymbolId, segments: &[String], line: u32) -> CallOutcome {
        let Some(name) = segments.last() else {
            return CallOutcome::External;
        };
        // Local bindings shadow items of the same name.
        if segments.len() == 1 && self.table.has_binding(caller, name, line) {
            return CallOutcome::Local;
        }
        match self.resolve_path(caller, segments, Ns::Value, 0) {
            PathResult::Found(found) => single_or_ambiguous(found.targets, found.via),
            PathResult::AssocMissing { owner } => {
                let is_enum = self.table.kind(&owner) == Some(SymbolKind::Enum);
                if is_enum && starts_uppercase(name) {
                    CallOutcome::Constructor
                } else {
                    CallOutcome::External
                }
            }
            PathResult::External => CallOutcome::External,
            PathResult::Missing { index: 0 } if segments.len() == 1 => {
                if self.bound_to_external_import(caller, name) {
                    return CallOutcome::External;
                }
                let names_struct =
                    self.lookup_lexical(caller, name, Ns::Type, 0)
                        .is_some_and(|found| {
                            found
                                .targets
                                .iter()
                                .any(|t| self.table.kind(t) == Some(SymbolKind::Struct))
                        });
                if names_struct {
                    CallOutcome::Constructor
                } else if self.table.is_function_name(name) {
                    CallOutcome::Unresolved(UnresolvedReason::NameNotInScope)
                } else {
                    CallOutcome::External
                }
            }
            PathResult::Missing { index: 0 } => {
                let root = &segments[0];
                if self.table.is_type_param(caller, root) {
                    self.ambiguous_by_name(name, AmbiguityReason::GenericParameter)
                } else if self.is_external_root(caller, root) || !self.table.is_callable_name(name)
                {
                    CallOutcome::External
                } else if root == "Self" {
                    CallOutcome::Unresolved(UnresolvedReason::SelfTypeUnknown)
                } else {
                    CallOutcome::Unresolved(UnresolvedReason::NameNotInScope)
                }
            }
            PathResult::Missing { .. } => {
                // `module::TupleStruct(..)`
                let names_struct = self
                    .resolve_unique(caller, segments, &[SymbolKind::Struct])
                    .is_some();
                if names_struct {
                    CallOutcome::Constructor
                } else {
                    CallOutcome::Unresolved(UnresolvedReason::PathSegmentNotFound)
                }
            }
        }
    }

    /// `<Type as Trait>::name(..)`
    fn resolve_qualified_call(
        &self,
        caller: &SymbolId,
        segments: &[String],
        trait_path: &[String],
    ) -> CallOutcome {
        let Some((name, type_path)) = segments.split_last() else {
            return CallOutcome::External;
        };
        let Some(tr) = self.resolve_unique(caller, trait_path, &[SymbolKind::Trait]) else {
            return CallOutcome::External;
        };
        let ty = self.resolve_unique(caller, type_path, &[SymbolKind::Struct, SymbolKind::Enum]);
        if let Some(ty) = ty {
            let implementing: Vec<SymbolId> = self
                .table
                .children_named(&ty, name)
                .iter()
                .filter(|m| {
                    self.method_trait
                        .get(&m.id)
                        .is_some_and(|t| self.table.parent(t) == Some(&tr))
                })
                .map(|m| m.id.clone())
                .collect();
            if !implementing.is_empty() {
                return single_or_ambiguous(implementing, ResolutionMethod::Path);
            }
        }
        let declared: Vec<SymbolId> = self
            .table
            .children_named(&tr, name)
            .iter()
            .filter(|s| s.kind == SymbolKind::Method)
            .map(|s| s.id.clone())
            .collect();
        if declared.is_empty() {
            CallOutcome::External
        } else {
            single_or_ambiguous(declared, ResolutionMethod::Path)
        }
    }

    fn resolve_method_call(
        &self,
        caller: &SymbolId,
        receiver: &Receiver,
        name: &str,
        line: u32,
    ) -> CallOutcome {
        let candidates = self.table.methods_named(name);
        if candidates.is_empty() {
            return CallOutcome::External;
        }
        match self.receiver_type(caller, receiver, line) {
            Inferred::Repo(ty) => {
                let via = match receiver {
                    Receiver::SelfValue | Receiver::SelfField(_) => ResolutionMethod::SelfType,
                    _ => ResolutionMethod::ReceiverType,
                };
                let methods = self.associated(&ty, name);
                if methods.is_empty() {
                    CallOutcome::External
                } else {
                    single_or_ambiguous(methods, via)
                }
            }
            Inferred::Foreign => CallOutcome::External,
            Inferred::Unknown => {
                let foreign_only = candidates.iter().all(|id| {
                    self.table.is_trait_impl_method(id) && !self.method_trait.contains_key(*id)
                });
                let reason = if foreign_only {
                    AmbiguityReason::ForeignTraitMethod
                } else if self.receiver_is_generic(caller, receiver, line) {
                    AmbiguityReason::GenericParameter
                } else {
                    AmbiguityReason::ReceiverTypeUnknown
                };
                self.ambiguous_by_name(name, reason)
            }
        }
    }

    /// Every repository method named `name` as candidates.
    fn ambiguous_by_name(&self, name: &str, reason: AmbiguityReason) -> CallOutcome {
        let mut candidates: Vec<SymbolId> = self
            .table
            .methods_named(name)
            .iter()
            .map(|&id| id.clone())
            .collect();
        if candidates.is_empty() {
            return CallOutcome::External;
        }
        candidates.sort();
        CallOutcome::Ambiguous { candidates, reason }
    }

    pub fn resolve_import(&self, import: &Import) -> ImportOutcome {
        let path = &import.path;
        let namespaces: &[Ns] = if import.glob {
            &[Ns::Type]
        } else {
            &[Ns::Type, Ns::Value]
        };
        let mut targets = Vec::new();
        let mut missing_at = None;
        for &ns in namespaces {
            match self.resolve_path(&import.scope, path, ns, 0) {
                PathResult::Found(found) => targets.extend(found.targets),
                // `use Enum::Variant` / `use Type::assoc`: record the owner.
                PathResult::AssocMissing { owner } => targets.push(owner),
                PathResult::Missing { index } => {
                    missing_at = Some(missing_at.map_or(index, |m: usize| m.max(index)));
                }
                PathResult::External => return ImportOutcome::External,
            }
        }
        if !targets.is_empty() {
            targets.sort();
            targets.dedup();
            return ImportOutcome::Resolved(targets);
        }
        match missing_at {
            // The first segment is not a repository name: an external crate.
            Some(0) => ImportOutcome::External,
            _ => ImportOutcome::Unresolved,
        }
    }
}

fn single_or_ambiguous(mut targets: Vec<SymbolId>, via: ResolutionMethod) -> CallOutcome {
    if targets.len() == 1 {
        CallOutcome::Resolved {
            target: targets.remove(0),
            via,
        }
    } else {
        targets.sort();
        CallOutcome::Ambiguous {
            candidates: targets,
            reason: AmbiguityReason::MultipleDefinitions,
        }
    }
}

fn starts_uppercase(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}
