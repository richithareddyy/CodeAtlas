//! Symbol resolution: turns textual references (call paths, method calls,
//! `use` items, impl headers) into edges between symbol IDs.
//!
//! Every reference ends in exactly one outcome. Only statically justified
//! targets become edges; ambiguous and unresolved references are kept, with
//! a reason, so that nothing is silently dropped or guessed. See
//! `docs/symbol-resolution.md`.

mod infer;
mod outcome;
mod rehome;
mod scope;
mod table;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub use outcome::{AmbiguityReason, UnresolvedReason};

use crate::model::{
    CrateTarget, Edge, EdgeKind, FileAnalysis, ResolutionMethod, SymbolId, SymbolKind,
};
use outcome::{CallOutcome, ImportOutcome};
use scope::Resolver;
use table::SymbolTable;

/// Ambiguous calls list at most this many candidates; `candidate_count`
/// always holds the full number.
const MAX_CANDIDATES: usize = 25;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Resolution {
    /// Resolved CALLS, IMPORTS and IMPLEMENTS edges, aggregated per
    /// (kind, from, to) and sorted.
    pub edges: Vec<Edge>,
    pub ambiguous_calls: Vec<AmbiguousCall>,
    pub unresolved_calls: Vec<UnresolvedCall>,
    pub stats: ResolutionStats,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmbiguousCall {
    pub caller: SymbolId,
    pub callee: String,
    pub line: u32,
    pub reason: AmbiguityReason,
    pub candidates: Vec<SymbolId>,
    pub candidate_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedCall {
    pub caller: SymbolId,
    pub callee: String,
    pub line: u32,
    pub reason: UnresolvedReason,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResolutionStats {
    pub calls: CallStats,
    pub imports: OutcomeCounts,
    pub impl_blocks: OutcomeCounts,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CallStats {
    pub total: u32,
    pub resolved: u32,
    pub ambiguous: u32,
    pub unresolved: u32,
    pub external: u32,
    pub constructors: u32,
    /// Calls of locally bound closures or function values.
    pub local: u32,
    pub resolved_by: BTreeMap<ResolutionMethod, u32>,
    pub ambiguous_by: BTreeMap<AmbiguityReason, u32>,
    pub unresolved_by: BTreeMap<UnresolvedReason, u32>,
    /// `resolved / (resolved + ambiguous + unresolved)`: the share of call
    /// sites that may target repository code and were resolved to exactly
    /// one definition. External calls, constructors and local calls are
    /// excluded.
    pub resolution_rate: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeCounts {
    pub total: u32,
    pub resolved: u32,
    pub external: u32,
    pub unresolved: u32,
}

/// Resolves all references in `files`. Impl methods are first re-homed
/// under their self types, which may rename their IDs in `files`.
pub(crate) fn resolve(
    files: &mut [FileAnalysis],
    crates: &[CrateTarget],
    dependencies: &BTreeSet<String>,
) -> Resolution {
    let plan = {
        let table = SymbolTable::build(files, crates, dependencies);
        rehome::plan(&Resolver::new(&table), files)
    };
    rehome::apply(files, &plan);

    let table = SymbolTable::build(files, crates, dependencies);
    let mut resolver = Resolver::new(&table);
    let mut edges = EdgeSet::default();
    let mut stats = ResolutionStats::default();

    for file in files.iter() {
        for block in &file.impls {
            stats.impl_blocks.total += 1;
            let kinds = [SymbolKind::Struct, SymbolKind::Enum];
            let ty = resolver.resolve_unique(&block.scope, &block.self_type, &kinds);
            let Some(trait_path) = &block.trait_path else {
                count(&mut stats.impl_blocks, ty.is_some(), false);
                continue;
            };
            let tr = resolver.resolve_unique(&block.scope, trait_path, &[SymbolKind::Trait]);
            count(
                &mut stats.impl_blocks,
                ty.is_some() && tr.is_some(),
                tr.is_none(),
            );
            if let (Some(ty), Some(tr)) = (ty, tr) {
                resolver.register_trait_impl(&ty, &tr, &block.methods);
                edges.add(
                    EdgeKind::Implements,
                    &ty,
                    &tr,
                    ResolutionMethod::ImplBlock,
                    block.span.start_line,
                );
            }
        }
    }
    for (method, declared) in &resolver.method_trait {
        if let Some(symbol) = table.symbol(method) {
            edges.add(
                EdgeKind::Implements,
                method,
                declared,
                ResolutionMethod::ImplBlock,
                symbol.span.start_line,
            );
        }
    }

    for import in files.iter().flat_map(|f| &f.imports) {
        stats.imports.total += 1;
        match resolver.resolve_import(import) {
            ImportOutcome::Resolved(targets) => {
                stats.imports.resolved += 1;
                for target in targets {
                    edges.add(
                        EdgeKind::Imports,
                        &import.scope,
                        &target,
                        ResolutionMethod::Import,
                        import.line,
                    );
                }
            }
            ImportOutcome::External => stats.imports.external += 1,
            ImportOutcome::Unresolved => stats.imports.unresolved += 1,
        }
    }

    let mut ambiguous_calls = Vec::new();
    let mut unresolved_calls = Vec::new();
    let calls = &mut stats.calls;
    for call in files.iter().flat_map(|f| &f.calls) {
        calls.total += 1;
        match resolver.resolve_call(call) {
            CallOutcome::Resolved { target, via } => {
                calls.resolved += 1;
                *calls.resolved_by.entry(via).or_default() += 1;
                edges.add(EdgeKind::Calls, &call.caller, &target, via, call.line);
            }
            CallOutcome::Ambiguous { candidates, reason } => {
                calls.ambiguous += 1;
                *calls.ambiguous_by.entry(reason).or_default() += 1;
                ambiguous_calls.push(AmbiguousCall {
                    caller: call.caller.clone(),
                    callee: call.callee.display(),
                    line: call.line,
                    reason,
                    candidate_count: candidates.len() as u32,
                    candidates: candidates.into_iter().take(MAX_CANDIDATES).collect(),
                });
            }
            CallOutcome::Unresolved(reason) => {
                calls.unresolved += 1;
                *calls.unresolved_by.entry(reason).or_default() += 1;
                unresolved_calls.push(UnresolvedCall {
                    caller: call.caller.clone(),
                    callee: call.callee.display(),
                    line: call.line,
                    reason,
                });
            }
            CallOutcome::External => calls.external += 1,
            CallOutcome::Constructor => calls.constructors += 1,
            CallOutcome::Local => calls.local += 1,
        }
    }
    let in_repo = calls.resolved + calls.ambiguous + calls.unresolved;
    calls.resolution_rate = (in_repo > 0).then(|| f64::from(calls.resolved) / f64::from(in_repo));

    Resolution {
        edges: edges.into_edges(),
        ambiguous_calls,
        unresolved_calls,
        stats,
    }
}

fn count(counts: &mut OutcomeCounts, resolved: bool, external: bool) {
    if resolved {
        counts.resolved += 1;
    } else if external {
        counts.external += 1;
    } else {
        counts.unresolved += 1;
    }
}

/// Aggregates repeated references between the same pair of symbols.
#[derive(Default)]
struct EdgeSet {
    edges: BTreeMap<(EdgeKind, SymbolId, SymbolId), (ResolutionMethod, Vec<u32>)>,
}

impl EdgeSet {
    fn add(
        &mut self,
        kind: EdgeKind,
        from: &SymbolId,
        to: &SymbolId,
        via: ResolutionMethod,
        line: u32,
    ) {
        let entry = self
            .edges
            .entry((kind, from.clone(), to.clone()))
            .or_insert((via, Vec::new()));
        if !entry.1.contains(&line) {
            entry.1.push(line);
        }
    }

    fn into_edges(self) -> Vec<Edge> {
        self.edges
            .into_iter()
            .map(|((kind, from, to), (via, mut lines))| {
                lines.sort_unstable();
                Edge {
                    from,
                    to,
                    kind,
                    via,
                    lines,
                }
            })
            .collect()
    }
}
