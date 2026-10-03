//! Moves impl methods under the type they belong to.
//!
//! The extractor names a method after the module its `impl` block is
//! written in, because it cannot know where `impl crate::model::Invoice`
//! points. Once paths can be resolved, each method is re-parented to its
//! self type and renamed to the type's canonical path, so that
//! `Invoice::discounted` has the same ID wherever its impl block lives.

use std::collections::{HashMap, HashSet};

use super::scope::Resolver;
use crate::model::{FileAnalysis, SymbolId, SymbolKind};

pub(crate) struct Rehome {
    id: SymbolId,
    qualified_name: String,
    parent: SymbolId,
}

pub(crate) fn plan(resolver: &Resolver<'_>, files: &[FileAnalysis]) -> HashMap<SymbolId, Rehome> {
    let table = resolver.table;
    let mut taken: HashSet<SymbolId> = files
        .iter()
        .flat_map(|f| &f.symbols)
        .map(|s| s.id.clone())
        .collect();
    let mut plan = HashMap::new();

    for block in files.iter().flat_map(|f| &f.impls) {
        let Some(ty) = resolver.resolve_unique(
            &block.scope,
            &block.self_type,
            &[SymbolKind::Struct, SymbolKind::Enum],
        ) else {
            continue;
        };
        let Some(ty_symbol) = table.symbol(&ty) else {
            continue;
        };
        let type_module = ty_symbol
            .qualified_name
            .rsplit_once("::")
            .map_or("", |(module, _)| module);

        for method_id in &block.methods {
            let Some(method) = table.symbol(method_id) else {
                continue;
            };
            let qualified_name = if block.trait_path.is_some() {
                let Some(trait_text) = trait_text(&method.qualified_name, &method.name) else {
                    continue;
                };
                format!(
                    "{type_module}::<{} as {trait_text}>::{}",
                    ty_symbol.name, method.name
                )
            } else {
                format!("{}::{}", ty_symbol.qualified_name, method.name)
            };

            let mut id = method.id.clone();
            if qualified_name != method.qualified_name {
                let base = SymbolId::new(SymbolKind::Method, &qualified_name);
                id = base.clone();
                let mut n = 1;
                while taken.contains(&id) {
                    n += 1;
                    id = base.with_suffix(n);
                }
                taken.insert(id.clone());
            }
            plan.insert(
                method.id.clone(),
                Rehome {
                    id,
                    qualified_name,
                    parent: ty.clone(),
                },
            );
        }
    }
    plan
}

pub(crate) fn apply(files: &mut [FileAnalysis], plan: &HashMap<SymbolId, Rehome>) {
    let renames: HashMap<SymbolId, SymbolId> = plan
        .iter()
        .filter(|(old, r)| *old != &r.id)
        .map(|(old, r)| (old.clone(), r.id.clone()))
        .collect();
    for file in files.iter_mut() {
        for symbol in &mut file.symbols {
            if let Some(r) = plan.get(&symbol.id) {
                symbol.qualified_name = r.qualified_name.clone();
                symbol.parent = Some(r.parent.clone());
            }
        }
        if !renames.is_empty() {
            file.remap_ids(&renames);
        }
    }
}

/// Extracts `Trait<Args>` from `scope::<Type as Trait<Args>>::name`.
fn trait_text<'q>(qualified_name: &'q str, name: &str) -> Option<&'q str> {
    let head = qualified_name.strip_suffix(name)?.strip_suffix(">::")?;
    let start = head.rfind(" as ")? + 4;
    Some(&head[start..])
}

#[cfg(test)]
mod tests {
    use super::trait_text;

    #[test]
    fn extracts_trait_text_from_qualified_names() {
        assert_eq!(
            trait_text("a::b::<Money as From<Cents>>::from", "from"),
            Some("From<Cents>")
        );
        assert_eq!(
            trait_text("a::<X as std::fmt::Display>::fmt", "fmt"),
            Some("std::fmt::Display")
        );
        assert_eq!(trait_text("a::X::fmt", "fmt"), None);
    }
}
