//! Name and path resolution, modelled on Rust's rules:
//!
//! * Names are looked up in the innermost scope first (function bodies,
//!   then their enclosing items), stopping at the first module. Modules do
//!   not inherit names from their parents.
//! * Within a scope, items defined there shadow explicit `use` imports,
//!   which shadow glob imports.
//! * Paths starting with `crate`, `self`, `super`, `Self` or a library
//!   crate name are anchored explicitly.
//! * Types and modules live in the type namespace; functions in the value
//!   namespace. Intermediate path segments are always in the type namespace.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use super::table::SymbolTable;
use crate::model::{Import, ResolutionMethod, SymbolId, SymbolKind};

/// Recursion limit for chains of re-exports and glob imports.
const MAX_DEPTH: u8 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Ns {
    Type,
    Value,
}

impl Ns {
    fn admits(self, kind: SymbolKind) -> bool {
        match self {
            Ns::Type => matches!(
                kind,
                SymbolKind::Module | SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Trait
            ),
            Ns::Value => kind == SymbolKind::Function,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Lookup {
    pub targets: Vec<SymbolId>,
    pub via: ResolutionMethod,
}

#[derive(Debug, Clone)]
pub(crate) enum PathResult {
    Found(Lookup),
    /// The path reached a repository type that has no associated function
    /// with the final name (derived methods, foreign traits, enum variants).
    AssocMissing {
        owner: SymbolId,
    },
    /// Resolution failed at segment `index`.
    Missing {
        index: usize,
    },
    /// The path leads out of the repository through a re-export or type
    /// alias of an external item (`pub use dep::f;`, `type Map = HashMap<..>`).
    External,
}

type CacheKey = (SymbolId, String, Ns, bool);

pub(crate) struct Resolver<'a> {
    pub table: &'a SymbolTable<'a>,
    /// Traits each type implements, from `impl Trait for Type` blocks.
    type_traits: HashMap<SymbolId, Vec<SymbolId>>,
    /// Trait method each trait-impl method implements.
    pub method_trait: HashMap<SymbolId, SymbolId>,
    /// Lookups that did not depend on a cut-off cycle; valid for good.
    cache: RefCell<HashMap<CacheKey, Option<Lookup>>>,
    /// Lookups in progress (the current chain of nested lookups). Meeting
    /// one again means a cycle of imports (e.g. two modules importing each
    /// other with `*`); that branch is cut off.
    in_progress: RefCell<HashSet<CacheKey>>,
    /// Lookups that depended on a cut (a cycle or the depth limit). Their
    /// result may differ in another context, so they are reused only within
    /// the current top-level lookup, which keeps it polynomial.
    provisional: RefCell<HashMap<CacheKey, Option<Lookup>>>,
    /// Set when the current lookup depended on a cut.
    cut: Cell<bool>,
}

impl<'a> Resolver<'a> {
    pub fn new(table: &'a SymbolTable<'a>) -> Self {
        Self {
            table,
            type_traits: HashMap::new(),
            method_trait: HashMap::new(),
            cache: RefCell::new(HashMap::new()),
            in_progress: RefCell::new(HashSet::new()),
            provisional: RefCell::new(HashMap::new()),
            cut: Cell::new(false),
        }
    }

    pub fn register_trait_impl(&mut self, ty: &SymbolId, tr: &SymbolId, methods: &[SymbolId]) {
        self.type_traits
            .entry(ty.clone())
            .or_default()
            .push(tr.clone());
        for method in methods {
            let Some(symbol) = self.table.symbol(method) else {
                continue;
            };
            let declared = self
                .table
                .children_named(tr, &symbol.name)
                .iter()
                .find(|s| s.kind == SymbolKind::Method);
            if let Some(declared) = declared {
                self.method_trait
                    .insert(method.clone(), declared.id.clone());
            }
        }
    }

    /// Resolves a path to exactly one symbol of one of `kinds`.
    pub fn resolve_unique(
        &self,
        scope: &SymbolId,
        path: &[String],
        kinds: &[SymbolKind],
    ) -> Option<SymbolId> {
        match self.resolve_path(scope, path, Ns::Type, 0) {
            PathResult::Found(lookup) => match lookup.targets.as_slice() {
                [only] if self.table.kind(only).is_some_and(|k| kinds.contains(&k)) => {
                    Some(only.clone())
                }
                _ => None,
            },
            _ => None,
        }
    }

    pub fn resolve_path(&self, scope: &SymbolId, path: &[String], ns: Ns, depth: u8) -> PathResult {
        let Some(first) = path.first() else {
            return PathResult::Missing { index: 0 };
        };
        let last = path.len() - 1;
        let missing = PathResult::Missing { index: 0 };

        let (mut current, via, mut index) = match first.as_str() {
            "crate" => match self.table.crate_root(scope) {
                Some(root) => (vec![root.clone()], ResolutionMethod::Path, 1),
                None => return missing,
            },
            "self" => match self.table.module_of(scope) {
                Some(module) => (vec![module.clone()], ResolutionMethod::Path, 1),
                None => return missing,
            },
            "super" => {
                let Some(mut module) = self.table.module_of(scope) else {
                    return missing;
                };
                let mut index = 0;
                while path.get(index).is_some_and(|s| s == "super") {
                    match self
                        .table
                        .parent(module)
                        .and_then(|p| self.table.module_of(p))
                    {
                        Some(parent) => module = parent,
                        None => return PathResult::Missing { index },
                    }
                    index += 1;
                }
                (vec![module.clone()], ResolutionMethod::Path, index)
            }
            "Self" => match self.table.self_type(scope) {
                Some(ty) => (vec![ty.clone()], ResolutionMethod::SelfType, 1),
                None => return missing,
            },
            // `::name` — a global path naming a crate.
            "" => match path.get(1).and_then(|name| self.table.lib_root(name)) {
                Some(root) => (vec![root.clone()], ResolutionMethod::Path, 2),
                None => return PathResult::External,
            },
            name => {
                let first_ns = if last == 0 { ns } else { Ns::Type };
                match self.lookup_lexical(scope, name, first_ns, depth) {
                    Some(lookup) => (lookup.targets, lookup.via, 1),
                    None => return missing,
                }
            }
        };

        while index <= last {
            let segment = &path[index];
            let segment_ns = if index == last { ns } else { Ns::Type };
            let mut next = Vec::new();
            let mut assoc_owner = None;
            for target in &current {
                match self.table.kind(target) {
                    Some(SymbolKind::Module) => {
                        // Private items and imports of a module are visible
                        // from inside it and its descendants.
                        let inside = self.table.is_enclosing_module(target, scope);
                        if let Some(found) =
                            self.lookup_in(target, segment, segment_ns, inside, depth)
                        {
                            next.extend(found.targets);
                        } else if self.binds_external(target, segment, depth) {
                            return PathResult::External;
                        }
                    }
                    Some(kind) if kind.is_type() => {
                        let methods = match segment_ns {
                            Ns::Value => self.associated(target, segment),
                            Ns::Type => vec![],
                        };
                        if methods.is_empty() {
                            assoc_owner = Some(target.clone());
                        }
                        next.extend(methods);
                    }
                    _ => {}
                }
            }
            if next.is_empty() {
                return match assoc_owner {
                    Some(owner) if index == last => PathResult::AssocMissing { owner },
                    _ => PathResult::Missing { index },
                };
            }
            dedup(&mut next);
            current = next;
            index += 1;
        }

        PathResult::Found(Lookup {
            targets: current,
            via,
        })
    }

    /// Looks `name` up from inside `scope`, walking outwards through
    /// enclosing functions to the nearest module, then library crates.
    pub fn lookup_lexical(
        &self,
        scope: &SymbolId,
        name: &str,
        ns: Ns,
        depth: u8,
    ) -> Option<Lookup> {
        let mut current = Some(scope);
        while let Some(s) = current {
            if let Some(found) = self.lookup_in(s, name, ns, true, depth) {
                return Some(found);
            }
            if self.table.kind(s) == Some(SymbolKind::Module) {
                break;
            }
            current = self.table.lexical_parent(s);
        }
        if ns == Ns::Type {
            if let Some(root) = self.table.lib_root(name) {
                if self.table.crate_root(scope) != Some(root) {
                    return Some(Lookup {
                        targets: vec![root.clone()],
                        via: ResolutionMethod::Path,
                    });
                }
            }
        }
        None
    }

    /// Names visible in `scope` itself. `lexical` is true when looking from
    /// inside the scope (all imports apply) and false for path access from
    /// outside (only `pub use` re-exports apply).
    pub fn lookup_in(
        &self,
        scope: &SymbolId,
        name: &str,
        ns: Ns,
        lexical: bool,
        depth: u8,
    ) -> Option<Lookup> {
        if depth > MAX_DEPTH {
            self.cut.set(true);
            return None;
        }
        let key = (scope.clone(), name.to_string(), ns, lexical);
        if let Some(cached) = self.cache.borrow().get(&key) {
            return cached.clone();
        }
        if let Some(provisional) = self.provisional.borrow().get(&key) {
            self.cut.set(true);
            return provisional.clone();
        }
        if self.in_progress.borrow().contains(&key) {
            self.cut.set(true);
            return None;
        }

        let top_level = self.in_progress.borrow().is_empty();
        self.in_progress.borrow_mut().insert(key.clone());
        let outer_cut = self.cut.replace(false);
        let result = self.lookup_in_uncached(scope, name, ns, lexical, depth);
        self.in_progress.borrow_mut().remove(&key);
        let cut = self.cut.get();
        if cut {
            self.provisional.borrow_mut().insert(key, result.clone());
        } else {
            self.cache.borrow_mut().insert(key, result.clone());
        }
        self.cut.set(outer_cut || cut);
        if top_level {
            self.provisional.borrow_mut().clear();
        }
        result
    }

    fn lookup_in_uncached(
        &self,
        scope: &SymbolId,
        name: &str,
        ns: Ns,
        lexical: bool,
        depth: u8,
    ) -> Option<Lookup> {
        let items: Vec<SymbolId> = self
            .table
            .children_named(scope, name)
            .iter()
            .filter(|s| ns.admits(s.kind))
            .map(|s| s.id.clone())
            .collect();
        if !items.is_empty() {
            return Some(Lookup {
                targets: items,
                via: ResolutionMethod::Scope,
            });
        }
        if ns == Ns::Type {
            if let Some(alias) = self.table.type_alias(scope, name) {
                if let PathResult::Found(found) =
                    self.resolve_path(scope, &alias.target.path, Ns::Type, depth + 1)
                {
                    return Some(Lookup {
                        targets: found.targets,
                        via: ResolutionMethod::Scope,
                    });
                }
            }
        }

        let visible =
            |import: &&&Import| lexical || import.visibility != crate::model::Visibility::Private;
        let imports = self.table.imports_in(scope);

        let mut targets = Vec::new();
        for import in imports
            .iter()
            .filter(visible)
            .filter(|i| i.bound_name() == Some(name))
        {
            if let PathResult::Found(found) = self.resolve_path(scope, &import.path, ns, depth + 1)
            {
                targets.extend(found.targets);
            }
        }
        if targets.is_empty() {
            for import in imports.iter().filter(visible).filter(|i| i.glob) {
                let PathResult::Found(bases) =
                    self.resolve_path(scope, &import.path, Ns::Type, depth + 1)
                else {
                    continue;
                };
                for base in bases.targets {
                    if self.table.kind(&base) == Some(SymbolKind::Module) {
                        // `use super::*` also brings in the parent's private
                        // items and imports, which the child can see.
                        let inside = self.table.is_enclosing_module(&base, scope);
                        if let Some(found) = self.lookup_in(&base, name, ns, inside, depth + 1) {
                            targets.extend(found.targets);
                        }
                    }
                }
            }
        }
        if targets.is_empty() {
            return None;
        }
        dedup(&mut targets);
        Some(Lookup {
            targets,
            via: ResolutionMethod::Import,
        })
    }

    /// Whether `module` binds `name` to something outside the repository:
    /// an import of an external item, or a type alias of an external type.
    fn binds_external(&self, module: &SymbolId, name: &str, depth: u8) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        let external_import = self
            .table
            .imports_in(module)
            .iter()
            .filter(|i| i.bound_name() == Some(name))
            .any(|i| {
                matches!(
                    self.resolve_import(i),
                    super::outcome::ImportOutcome::External
                )
            });
        external_import
            || self.table.type_alias(module, name).is_some_and(|alias| {
                !matches!(
                    self.resolve_path(module, &alias.target.path, Ns::Type, depth + 1),
                    PathResult::Found(_)
                )
            })
    }

    /// Associated functions named `name` on a struct, enum or trait,
    /// following Rust's priority: inherent methods, then methods from trait
    /// impls for the type, then default methods of implemented traits.
    pub fn associated(&self, ty: &SymbolId, name: &str) -> Vec<SymbolId> {
        let methods: Vec<&SymbolId> = self
            .table
            .children_named(ty, name)
            .iter()
            .filter(|s| s.kind == SymbolKind::Method)
            .map(|s| &s.id)
            .collect();
        if self.table.kind(ty) == Some(SymbolKind::Trait) {
            return methods.into_iter().cloned().collect();
        }
        let (from_traits, inherent): (Vec<_>, Vec<_>) = methods
            .into_iter()
            .partition(|m| self.table.is_trait_impl_method(m));
        if !inherent.is_empty() {
            return inherent.into_iter().cloned().collect();
        }
        if !from_traits.is_empty() {
            return from_traits.into_iter().cloned().collect();
        }
        self.type_traits
            .get(ty)
            .into_iter()
            .flatten()
            .flat_map(|tr| self.table.children_named(tr, name))
            .filter(|s| s.kind == SymbolKind::Method)
            .map(|s| s.id.clone())
            .collect()
    }
}

fn dedup(ids: &mut Vec<SymbolId>) {
    ids.sort();
    ids.dedup();
}
