//! Test selection strategies compared on the same probe ground truth.
//!
//! Besides CodeAtlas's own selection, three simple baselines are scored:
//! every test of the changed Cargo package and the workspace packages
//! depending on it, every test in files that depend on the changed file,
//! and every test binary whose sources mention the changed function's
//! name. Whole-suite recall is the primary measure: its denominator is
//! every failing test, including tests the analysis cannot see.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::error::{AnalyzerError, Result};
use crate::graph::impact::ImpactOptions;
use crate::graph::test_selection::select_tests;
use crate::graph::CodeGraph;
use crate::model::{SymbolId, TargetKind};
use crate::parser::RustParser;
use crate::test_evaluation::{ProbeStatus, TestGroundTruth, UNMAPPED};
use crate::RepositoryAnalysis;

pub const STRATEGIES: [&str; 7] = [
    "run-all",
    "changed-package",
    "file-dependency",
    "name-mention",
    "codeatlas",
    "codeatlas-ambiguous",
    "codeatlas-conservative",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyComparison {
    pub repository: String,
    pub commit: Option<String>,
    pub max_depth: u32,
    /// Tests that passed in both baseline runs and are not flaky.
    pub tests: u32,
    /// Of those, tests with no symbol in the analysis.
    pub invisible_tests: u32,
    pub flaky_tests: u32,
    pub probes: ProbeCounts,
    pub resolution: ResolutionCoverage,
    /// How often each conservative-rule reason fired, over `ok` probes.
    pub reasons: BTreeMap<String, u32>,
    pub strategies: Vec<StrategySummary>,
    pub per_probe: Vec<ProbeComparison>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeCounts {
    pub total: u32,
    pub ok: u32,
    pub build_failed: u32,
    pub incomplete: u32,
    /// `ok` probes whose function no test executed.
    pub not_executed: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolutionCoverage {
    pub call_sites: u32,
    pub resolved: u32,
    pub ambiguous: u32,
    pub unresolved: u32,
    pub resolution_rate: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategySummary {
    pub name: String,
    /// `ok` probes with at least one failing test.
    pub probes_with_failures: u32,
    pub failing: u32,
    pub true_positives: u32,
    /// Whole-suite recall: selected failing tests over all failing tests.
    pub recall: Option<f64>,
    /// Mean per-probe recall over probes with failures.
    pub mean_recall: Option<f64>,
    /// Probes with failures in which no failing test was missed.
    pub safe_probes: u32,
    pub precision: Option<f64>,
    /// Means over `ok` probes.
    pub mean_selected: f64,
    pub mean_selected_share: f64,
    /// Mean of `1 - selected test time / suite test time`, from libtest's
    /// per-test times; `None` without times.
    pub mean_time_saved: Option<f64>,
    pub missed_invisible: u32,
    pub missed_visible: u32,
    /// Secondary: recall over failing tests that have a symbol.
    pub recall_visible: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeComparison {
    pub symbol: String,
    pub failing: u32,
    pub failing_invisible: u32,
    /// Conservative rule: whether selective execution was allowed.
    pub selective: bool,
    pub reasons: Vec<String>,
    pub outcomes: BTreeMap<String, ProbeOutcome>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeOutcome {
    pub selected: u32,
    pub true_positives: u32,
    pub missed_invisible: u32,
    /// Failing tests with a symbol that were not selected.
    pub missed_visible: Vec<String>,
    pub time_share: Option<f64>,
}

pub fn compare_strategies(
    analysis: &RepositoryAnalysis,
    truth: &TestGroundTruth,
    max_depth: u32,
) -> Result<StrategyComparison> {
    let graph = CodeGraph::from_analysis(analysis);
    let excluded: HashSet<&String> = truth.baseline_failures.iter().chain(&truth.flaky).collect();
    let suite: BTreeSet<&str> = truth
        .tests
        .iter()
        .filter(|t| !excluded.contains(t))
        .map(String::as_str)
        .collect();
    let invisible = |t: &str| t.starts_with(UNMAPPED);
    let test_file: HashMap<&str, &str> = graph
        .symbols()
        .iter()
        .filter(|s| s.is_test)
        .map(|s| (s.id.as_str(), s.file.as_str()))
        .collect();
    let suite_seconds: f64 = suite
        .iter()
        .filter_map(|t| truth.test_seconds.get(*t))
        .sum();
    let names = identifier_index(analysis)?;

    let mut counts = ProbeCounts {
        total: truth.probes.len() as u32,
        ..Default::default()
    };
    let mut reasons: BTreeMap<String, u32> = BTreeMap::new();
    let mut per_probe = Vec::new();
    for probe in &truth.probes {
        match probe.status {
            ProbeStatus::Ok => counts.ok += 1,
            ProbeStatus::BuildFailed => {
                counts.build_failed += 1;
                continue;
            }
            ProbeStatus::Incomplete => {
                counts.incomplete += 1;
                continue;
            }
        }
        let failing: BTreeSet<&str> = probe
            .failing
            .iter()
            .map(String::as_str)
            .filter(|t| suite.contains(t))
            .collect();
        if failing.is_empty() {
            counts.not_executed += 1;
        }
        let id = SymbolId::from_stored(probe.symbol.clone());
        let node = graph.node(&id);
        let symbol = node.map(|n| graph.symbol(n));

        let mut selections: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        selections.insert("run-all", suite.clone());
        // Older ground truths have no package data; the strategy is left out.
        if !truth.packages.is_empty() {
            let packages = changed_packages(analysis, truth, symbol.map(|s| s.file.as_str()))
                .unwrap_or_default();
            selections.insert(
                "changed-package",
                suite
                    .iter()
                    .copied()
                    .filter(|t| {
                        truth
                            .test_packages
                            .get(*t)
                            .is_some_and(|p| packages.contains(p.as_str()))
                    })
                    .collect(),
            );
        }
        let files = symbol
            .map(|s| dependent_files(&graph, &s.file))
            .unwrap_or_default();
        selections.insert(
            "file-dependency",
            suite
                .iter()
                .copied()
                .filter(|t| test_file.get(t).is_some_and(|f| files.contains(*f)))
                .collect(),
        );
        let crates: HashSet<&str> = symbol
            .and_then(|s| names.get(s.name.as_str()))
            .map(|crates| crates.iter().map(String::as_str).collect())
            .unwrap_or_default();
        selections.insert(
            "name-mention",
            suite
                .iter()
                .copied()
                .filter(|t| test_crate(t).is_some_and(|c| crates.contains(c)))
                .collect(),
        );

        let mut selective = false;
        let mut probe_reasons = Vec::new();
        for (name, ambiguous) in [("codeatlas", false), ("codeatlas-ambiguous", true)] {
            let options = ImpactOptions {
                max_depth,
                include_ambiguous: ambiguous,
                ..Default::default()
            };
            let mut chosen: BTreeSet<&str> = BTreeSet::new();
            if let Some(selection) = node.and_then(|n| select_tests(&graph, &[n], options).ok()) {
                let mut ids: Vec<String> =
                    selection.to_run().iter().map(|t| t.to_string()).collect();
                if ambiguous {
                    ids.extend(selection.possible.iter().map(|t| t.test.id.to_string()));
                } else {
                    selective = selection.safety.selective;
                    probe_reasons = selection
                        .safety
                        .reasons
                        .iter()
                        .map(|r| r.kind().to_string())
                        .collect();
                }
                chosen = ids
                    .iter()
                    .filter_map(|t| suite.get(t.as_str()).copied())
                    .collect();
            }
            selections.insert(name, chosen);
        }
        let conservative = if selective {
            selections["codeatlas"].clone()
        } else {
            suite.clone()
        };
        selections.insert("codeatlas-conservative", conservative);
        for r in probe_reasons.iter().collect::<BTreeSet<_>>() {
            *reasons.entry(r.clone()).or_default() += 1;
        }

        let outcomes = selections
            .into_iter()
            .map(|(name, selected)| {
                let time: f64 = selected
                    .iter()
                    .filter_map(|t| truth.test_seconds.get(*t))
                    .sum();
                let missed: Vec<&str> = failing.difference(&selected).copied().collect();
                (
                    name.to_string(),
                    ProbeOutcome {
                        selected: selected.len() as u32,
                        true_positives: failing.intersection(&selected).count() as u32,
                        missed_invisible: missed.iter().filter(|t| invisible(t)).count() as u32,
                        missed_visible: missed
                            .iter()
                            .filter(|t| !invisible(t))
                            .map(|t| t.to_string())
                            .collect(),
                        time_share: (suite_seconds > 0.0).then(|| round(time / suite_seconds)),
                    },
                )
            })
            .collect();
        per_probe.push(ProbeComparison {
            symbol: probe.symbol.clone(),
            failing: failing.len() as u32,
            failing_invisible: failing.iter().filter(|t| invisible(t)).count() as u32,
            selective,
            reasons: probe_reasons,
            outcomes,
        });
    }

    let strategies = STRATEGIES
        .iter()
        .filter(|name| per_probe.iter().all(|p| p.outcomes.contains_key(**name)))
        .map(|name| summarize(name, &per_probe, suite.len()))
        .collect();
    let calls = &analysis.resolution.stats.calls;
    Ok(StrategyComparison {
        repository: truth.repository.clone(),
        commit: truth.commit.clone(),
        max_depth,
        tests: suite.len() as u32,
        invisible_tests: suite.iter().filter(|t| invisible(t)).count() as u32,
        flaky_tests: truth.flaky.len() as u32,
        probes: counts,
        resolution: ResolutionCoverage {
            call_sites: calls.total,
            resolved: calls.resolved,
            ambiguous: calls.ambiguous,
            unresolved: calls.unresolved,
            resolution_rate: calls.resolution_rate.map(round),
        },
        reasons,
        strategies,
        per_probe,
    })
}

fn summarize(name: &str, probes: &[ProbeComparison], suite: usize) -> StrategySummary {
    let outcomes: Vec<(&ProbeComparison, &ProbeOutcome)> =
        probes.iter().map(|p| (p, &p.outcomes[name])).collect();
    let with_failures: Vec<&(&ProbeComparison, &ProbeOutcome)> =
        outcomes.iter().filter(|(p, _)| p.failing > 0).collect();
    let failing: u32 = with_failures.iter().map(|(p, _)| p.failing).sum();
    let true_positives: u32 = with_failures.iter().map(|(_, o)| o.true_positives).sum();
    let selected: u32 = outcomes.iter().map(|(_, o)| o.selected).sum();
    let missed_invisible: u32 = outcomes.iter().map(|(_, o)| o.missed_invisible).sum();
    let missed_visible: u32 = outcomes
        .iter()
        .map(|(_, o)| o.missed_visible.len() as u32)
        .sum();
    let visible_failing: u32 = with_failures
        .iter()
        .map(|(p, _)| p.failing - p.failing_invisible)
        .sum();
    let n = outcomes.len().max(1) as f64;
    let times: Vec<f64> = outcomes.iter().filter_map(|(_, o)| o.time_share).collect();
    StrategySummary {
        name: name.to_string(),
        probes_with_failures: with_failures.len() as u32,
        failing,
        true_positives,
        recall: ratio(true_positives, failing),
        mean_recall: (!with_failures.is_empty()).then(|| {
            round(
                with_failures
                    .iter()
                    .map(|(p, o)| f64::from(o.true_positives) / f64::from(p.failing))
                    .sum::<f64>()
                    / with_failures.len() as f64,
            )
        }),
        safe_probes: with_failures
            .iter()
            .filter(|(p, o)| o.true_positives == p.failing)
            .count() as u32,
        precision: ratio(true_positives, selected),
        mean_selected: round(f64::from(selected) / n),
        mean_selected_share: round(f64::from(selected) / n / suite.max(1) as f64),
        mean_time_saved: (!times.is_empty())
            .then(|| round(1.0 - times.iter().sum::<f64>() / times.len() as f64)),
        missed_invisible,
        missed_visible,
        recall_visible: ratio(visible_failing - missed_visible, visible_failing),
    }
}

/// The changed file and every file that depends on it, transitively.
fn dependent_files(graph: &CodeGraph, file: &str) -> HashSet<String> {
    let mut found: HashSet<String> = HashSet::from([file.to_string()]);
    let mut frontier = vec![file.to_string()];
    while let Some(current) = frontier.pop() {
        for d in &graph.dependencies().files {
            if d.to == current && found.insert(d.from.clone()) {
                frontier.push(d.from.clone());
            }
        }
    }
    found
}

/// The package containing `file` and the workspace packages that depend on
/// it, transitively.
fn changed_packages<'t>(
    analysis: &RepositoryAnalysis,
    truth: &'t TestGroundTruth,
    file: Option<&str>,
) -> Option<HashSet<&'t str>> {
    let crate_name = analysis
        .files
        .iter()
        .find(|f| Some(f.path.as_str()) == file)?
        .crate_name
        .as_str();
    let package = analysis
        .crates
        .iter()
        .filter(|c| c.name == crate_name)
        .min_by_key(|c| !matches!(c.kind, TargetKind::Lib | TargetKind::Bin))?
        .package
        .as_str();
    let mut found: HashSet<&str> = truth
        .packages
        .iter()
        .filter(|p| p.name == package)
        .map(|p| p.name.as_str())
        .collect();
    loop {
        let more: Vec<&str> = truth
            .packages
            .iter()
            .filter(|p| !found.contains(p.name.as_str()))
            .filter(|p| p.dependencies.iter().any(|d| found.contains(d.as_str())))
            .map(|p| p.name.as_str())
            .collect();
        if more.is_empty() {
            return Some(found);
        }
        found.extend(more);
    }
}

/// Crate name of a test ID (`fn:crate::...`, `unmapped:crate::...`).
fn test_crate(test: &str) -> Option<&str> {
    let rest = test
        .strip_prefix("fn:")
        .or_else(|| test.strip_prefix(UNMAPPED))?;
    rest.split("::").next()
}

/// Crates whose files contain each identifier token (code and macro
/// tokens, not comments or strings).
fn identifier_index(analysis: &RepositoryAnalysis) -> Result<HashMap<String, BTreeSet<String>>> {
    let mut parser = RustParser::new()?;
    let mut index: HashMap<String, BTreeSet<String>> = HashMap::new();
    for file in &analysis.files {
        let path = analysis.repository.root.join(&file.path);
        let src = std::fs::read_to_string(&path).map_err(|err| AnalyzerError::io(&path, err))?;
        let tree = parser.parse(&src, &file.path)?;
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            if node.kind() == "identifier" {
                index
                    .entry(src[node.byte_range()].to_string())
                    .or_default()
                    .insert(file.crate_name.clone());
                continue;
            }
            let mut cursor = node.walk();
            stack.extend(node.children(&mut cursor));
        }
    }
    Ok(index)
}

fn ratio(part: u32, whole: u32) -> Option<f64> {
    (whole > 0).then(|| round(f64::from(part) / f64::from(whole)))
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}
