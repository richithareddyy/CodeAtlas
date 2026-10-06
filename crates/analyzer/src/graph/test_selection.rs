//! Test selection: which tests could be affected by changed symbols.
//!
//! A test is selected when the impact engine reaches it from a changed
//! symbol (calls, trait dispatch, implementations of changed trait methods;
//! ambiguous calls only on request, as *possible*). Each selected test
//! keeps the evidence chain to the change. A test is *direct* when that
//! chain contains a single call (dispatch and implementation steps do not
//! count: a test calling `Gateway::charge` directly exercises a changed
//! `<Stripe as Gateway>::charge`), and *transitive* otherwise.
//!
//! The selection also reports changed tests (to run as well) and changed
//! functions that no test reaches, which are the gaps a reviewer should
//! know about.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::impact::{
    analyze_impact, AffectedSymbol, Confidence, DependencyKind, EvidenceStep, ImpactError,
    ImpactOptions, SymbolRef,
};
use super::CodeGraph;
use crate::model::SymbolId;

/// Above this many changed symbols, `reaches` and `untested` are not
/// computed (one traversal per changed symbol).
const MAX_PER_SYMBOL_TRAVERSALS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedTest {
    pub test: SymbolRef,
    /// Length of the evidence chain.
    pub depth: u32,
    /// Call steps in the chain (1 for direct tests).
    pub calls: u32,
    pub confidence: Confidence,
    /// From the test to a changed symbol.
    pub path: Vec<EvidenceStep>,
    /// Changed symbols this test reaches through resolved relationships.
    pub reaches: Vec<SymbolId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestSelection {
    /// Changed symbols (after expanding types and modules to members).
    pub changed: Vec<SymbolRef>,
    /// Tests that are themselves among the changed symbols.
    pub changed_tests: Vec<SymbolRef>,
    /// Tests calling a changed symbol directly.
    pub direct: Vec<SelectedTest>,
    /// Tests reaching a changed symbol through other code.
    pub transitive: Vec<SelectedTest>,
    /// Tests reached only through ambiguous calls (when requested).
    pub possible: Vec<SelectedTest>,
    /// Changed functions and methods that no test reaches.
    pub untested: Vec<SymbolRef>,
    /// Tests in the repository.
    pub total_tests: u32,
    /// Whether `reaches` and `untested` were computed.
    pub per_symbol: bool,
    pub truncated: bool,
}

impl TestSelection {
    /// Tests to run: changed tests and certain selections.
    pub fn to_run(&self) -> BTreeSet<&SymbolId> {
        self.changed_tests
            .iter()
            .map(|t| &t.id)
            .chain(self.direct.iter().map(|t| &t.test.id))
            .chain(self.transitive.iter().map(|t| &t.test.id))
            .collect()
    }
}

/// Selects the tests affected by changing the symbols at `changed` (node
/// indexes, as for [`analyze_impact`]).
pub fn select_tests(
    graph: &CodeGraph,
    changed: &[usize],
    options: ImpactOptions,
) -> Result<TestSelection, ImpactError> {
    let report = analyze_impact(graph, changed, options)?;
    let total_tests = graph.symbols().iter().filter(|s| s.is_test).count() as u32;

    // Which changed symbols each test reaches; one traversal per changed
    // callable, without ambiguous calls.
    let callables: Vec<usize> = report
        .changed
        .iter()
        .filter(|c| c.kind.is_callable() && !c.is_test)
        .filter_map(|c| graph.node(&c.id))
        .collect();
    let per_symbol = callables.len() <= MAX_PER_SYMBOL_TRAVERSALS;
    let mut reaches: BTreeMap<SymbolId, Vec<SymbolId>> = BTreeMap::new();
    let mut untested = Vec::new();
    let mut truncated = report.truncated;
    if per_symbol {
        let certain_only = ImpactOptions {
            include_ambiguous: false,
            ..options
        };
        for &node in &callables {
            let id = &graph.symbol(node).id;
            let single = analyze_impact(graph, &[node], certain_only)?;
            truncated |= single.truncated;
            if single.tests.is_empty() {
                if let Some(changed) = report.changed.iter().find(|c| &c.id == id) {
                    untested.push(changed.clone());
                }
            }
            for test in single.tests {
                reaches.entry(test).or_default().push(id.clone());
            }
        }
    }

    let (mut direct, mut transitive, mut possible) = (Vec::new(), Vec::new(), Vec::new());
    for a in report.affected.into_iter().filter(|a| a.symbol.is_test) {
        let selected = selected(a, &reaches);
        match (selected.confidence, selected.calls) {
            (Confidence::Possible, _) => possible.push(selected),
            (Confidence::Certain, 0 | 1) => direct.push(selected),
            (Confidence::Certain, _) => transitive.push(selected),
        }
    }
    let changed_tests = report
        .changed
        .iter()
        .filter(|c| c.is_test)
        .cloned()
        .collect();
    untested.sort_by(|a, b| a.id.cmp(&b.id));

    Ok(TestSelection {
        changed: report.changed,
        changed_tests,
        direct,
        transitive,
        possible,
        untested,
        total_tests,
        per_symbol,
        truncated,
    })
}

fn selected(a: AffectedSymbol, reaches: &BTreeMap<SymbolId, Vec<SymbolId>>) -> SelectedTest {
    let calls = a
        .path
        .iter()
        .filter(|s| matches!(s.kind, DependencyKind::Calls | DependencyKind::MayCall))
        .count() as u32;
    SelectedTest {
        reaches: reaches.get(&a.symbol.id).cloned().unwrap_or_default(),
        test: a.symbol,
        depth: a.depth,
        calls,
        confidence: a.confidence,
        path: a.path,
    }
}
