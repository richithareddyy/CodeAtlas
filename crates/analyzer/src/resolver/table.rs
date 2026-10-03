//! Read-only indexes over the extracted symbols of a whole repository.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::model::{
    CrateTarget, FileAnalysis, ImplBlock, Import, LocalBinding, Symbol, SymbolId, SymbolKind,
    TargetKind, TypeAlias, TypeRef,
};

pub(crate) struct SymbolTable<'a> {
    symbols: HashMap<&'a SymbolId, &'a Symbol>,
    /// Children of a symbol, keyed by parent and then name.
    named_children: HashMap<&'a SymbolId, HashMap<&'a str, Vec<&'a Symbol>>>,
    /// `use` items, keyed by the scope they are declared in.
    imports: HashMap<&'a SymbolId, Vec<&'a Import>>,
    bindings: HashMap<&'a SymbolId, Vec<&'a LocalBinding>>,
    fields: HashMap<&'a SymbolId, HashMap<&'a str, &'a TypeRef>>,
    type_aliases: HashMap<&'a SymbolId, HashMap<&'a str, &'a TypeAlias>>,
    /// Root module of each library target, by crate name.
    lib_roots: HashMap<&'a str, &'a SymbolId>,
    dependencies: &'a BTreeSet<String>,
    methods_by_name: HashMap<&'a str, Vec<&'a SymbolId>>,
    function_names: HashSet<&'a str>,
    /// Names of modules, structs, enums and traits anywhere in the repository.
    type_names: HashSet<&'a str>,
    /// Methods defined in `impl Trait for Type` blocks.
    trait_impl_methods: HashSet<&'a SymbolId>,
    /// Scope an impl method's body resolves names in: where its `impl`
    /// block is written, which can differ from its (re-homed) parent type.
    impl_scopes: HashMap<&'a SymbolId, &'a SymbolId>,
    pub impls: Vec<&'a ImplBlock>,
}

impl<'a> SymbolTable<'a> {
    pub fn build(
        files: &'a [FileAnalysis],
        crates: &'a [CrateTarget],
        dependencies: &'a BTreeSet<String>,
    ) -> Self {
        let mut table = SymbolTable {
            symbols: HashMap::new(),
            named_children: HashMap::new(),
            imports: HashMap::new(),
            bindings: HashMap::new(),
            fields: HashMap::new(),
            type_aliases: HashMap::new(),
            lib_roots: HashMap::new(),
            dependencies,
            methods_by_name: HashMap::new(),
            function_names: HashSet::new(),
            type_names: HashSet::new(),
            trait_impl_methods: HashSet::new(),
            impl_scopes: HashMap::new(),
            impls: Vec::new(),
        };

        for file in files {
            for symbol in &file.symbols {
                table.symbols.insert(&symbol.id, symbol);
                if let Some(parent) = &symbol.parent {
                    table
                        .named_children
                        .entry(parent)
                        .or_default()
                        .entry(&symbol.name)
                        .or_default()
                        .push(symbol);
                }
                match symbol.kind {
                    SymbolKind::Method => table
                        .methods_by_name
                        .entry(&symbol.name)
                        .or_default()
                        .push(&symbol.id),
                    SymbolKind::Function => {
                        table.function_names.insert(&symbol.name);
                    }
                    _ => {
                        table.type_names.insert(&symbol.name);
                    }
                }
            }
            for import in &file.imports {
                table.imports.entry(&import.scope).or_default().push(import);
            }
            for binding in &file.bindings {
                table
                    .bindings
                    .entry(&binding.scope)
                    .or_default()
                    .push(binding);
            }
            for field in &file.fields {
                table
                    .fields
                    .entry(&field.owner)
                    .or_default()
                    .insert(&field.name, &field.ty);
            }
            for alias in &file.type_aliases {
                table
                    .type_aliases
                    .entry(&alias.scope)
                    .or_default()
                    .insert(&alias.name, alias);
            }
            for block in &file.impls {
                if block.trait_path.is_some() {
                    table.trait_impl_methods.extend(&block.methods);
                }
                for method in &block.methods {
                    table.impl_scopes.insert(method, &block.scope);
                }
                table.impls.push(block);
            }
        }

        for target in crates.iter().filter(|t| t.kind == TargetKind::Lib) {
            let root = files
                .iter()
                .find(|f| f.path == target.root_file)
                .and_then(|f| f.module.as_ref());
            if let Some(root) = root {
                table.lib_roots.insert(&target.name, root);
            }
        }
        table
    }

    pub fn symbol(&self, id: &SymbolId) -> Option<&'a Symbol> {
        self.symbols.get(id).copied()
    }

    pub fn kind(&self, id: &SymbolId) -> Option<SymbolKind> {
        self.symbol(id).map(|s| s.kind)
    }

    pub fn parent(&self, id: &SymbolId) -> Option<&'a SymbolId> {
        self.symbol(id).and_then(|s| s.parent.as_ref())
    }

    pub fn children_named(&self, parent: &SymbolId, name: &str) -> &[&'a Symbol] {
        self.named_children
            .get(parent)
            .and_then(|children| children.get(name))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn imports_in(&self, scope: &SymbolId) -> &[&'a Import] {
        self.imports.get(scope).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn bindings_in(&self, scope: &SymbolId) -> &[&'a LocalBinding] {
        self.bindings.get(scope).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn field_type(&self, owner: &SymbolId, field: &str) -> Option<&'a TypeRef> {
        self.fields.get(owner)?.get(field).copied()
    }

    pub fn type_alias(&self, scope: &SymbolId, name: &str) -> Option<&'a TypeAlias> {
        self.type_aliases.get(scope)?.get(name).copied()
    }

    /// Whether `scope` has a local binding (parameter or `let`) named
    /// `name` at or before `line`.
    pub fn has_binding(&self, scope: &SymbolId, name: &str, line: u32) -> bool {
        self.bindings_in(scope)
            .iter()
            .any(|b| b.name == name && b.line <= line)
    }

    /// Whether `module` is `scope`'s module or one of its ancestors, i.e.
    /// whether private items of `module` are visible from `scope`.
    pub fn is_enclosing_module(&self, module: &SymbolId, scope: &SymbolId) -> bool {
        let mut current = self.module_of(scope);
        while let Some(m) = current {
            if m == module {
                return true;
            }
            current = self.parent(m).and_then(|p| self.module_of(p));
        }
        false
    }

    pub fn lib_root(&self, crate_name: &str) -> Option<&'a SymbolId> {
        self.lib_roots.get(crate_name).copied()
    }

    pub fn methods_named(&self, name: &str) -> &[&'a SymbolId] {
        self.methods_by_name
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn is_function_name(&self, name: &str) -> bool {
        self.function_names.contains(name)
    }

    /// Whether `name` is a declared dependency crate that is not also a
    /// library in the repository.
    pub fn is_dependency(&self, name: &str) -> bool {
        self.dependencies.contains(name) && !self.lib_roots.contains_key(name)
    }

    pub fn is_type_name(&self, name: &str) -> bool {
        self.type_names.contains(name) || self.lib_roots.contains_key(name)
    }

    /// Whether `name` is a generic type parameter visible in `scope`.
    pub fn is_type_param(&self, scope: &SymbolId, name: &str) -> bool {
        let mut current = Some(scope);
        while let Some(id) = current {
            let Some(symbol) = self.symbol(id) else {
                return false;
            };
            if symbol.type_params.iter().any(|p| p == name) {
                return true;
            }
            if symbol.kind == SymbolKind::Module {
                return false;
            }
            current = self.lexical_parent(id);
        }
        false
    }

    pub fn is_callable_name(&self, name: &str) -> bool {
        self.is_function_name(name) || self.methods_by_name.contains_key(name)
    }

    pub fn is_trait_impl_method(&self, id: &SymbolId) -> bool {
        self.trait_impl_methods.contains(id)
    }

    /// The enclosing scope for name lookup: the impl block's scope for impl
    /// methods, the declaring parent otherwise.
    pub fn lexical_parent(&self, id: &SymbolId) -> Option<&'a SymbolId> {
        self.impl_scopes
            .get(id)
            .copied()
            .or_else(|| self.parent(id))
    }

    /// Nearest lexically enclosing module (the symbol itself if it is a module).
    pub fn module_of(&self, id: &SymbolId) -> Option<&'a SymbolId> {
        let mut current = self.symbol(id)?;
        loop {
            if current.kind == SymbolKind::Module {
                return Some(&current.id);
            }
            current = self.symbol(self.lexical_parent(&current.id)?)?;
        }
    }

    /// Root module of the crate containing `id`.
    pub fn crate_root(&self, id: &SymbolId) -> Option<&'a SymbolId> {
        let mut module = self.module_of(id)?;
        while let Some(parent) = self.parent(module) {
            module = self.module_of(parent)?;
        }
        Some(module)
    }

    /// The type `Self` refers to inside `scope`: the owning type of a method,
    /// or the type itself.
    pub fn self_type(&self, scope: &SymbolId) -> Option<&'a SymbolId> {
        let symbol = self.symbol(scope)?;
        match symbol.kind {
            SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Trait => Some(&symbol.id),
            SymbolKind::Method => {
                let parent = symbol.parent.as_ref()?;
                self.kind(parent)
                    .is_some_and(SymbolKind::is_type)
                    .then_some(parent)
            }
            _ => None,
        }
    }
}
