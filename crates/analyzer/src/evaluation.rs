//! Measures resolution quality against hand-written ground truth.
//!
//! Ground truth lists every edge and every ambiguous or unresolved call
//! site a person expects after reading the source. Comparing it with the
//! resolver's output yields precision and recall per category, plus the
//! exact differences, so regressions are visible rather than averaged away.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::model::EdgeKind;
use crate::resolver::Resolution;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct GroundTruth {
    /// `[caller, callee]` pairs of resolved CALLS edges.
    #[serde(default)]
    pub calls: Vec<(String, String)>,
    /// `[implementor, trait or trait method]` pairs.
    #[serde(default)]
    pub implements: Vec<(String, String)>,
    #[serde(default)]
    pub ambiguous: Vec<ExpectedSite>,
    #[serde(default)]
    pub unresolved: Vec<ExpectedSite>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExpectedSite {
    pub caller: String,
    /// Callee as rendered by `Callee::display`, e.g. `job.run`.
    pub callee: String,
    /// Snake-case reason, e.g. `receiver_type_unknown`.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SetComparison {
    pub expected: usize,
    pub actual: usize,
    pub true_positives: usize,
    /// Expected but not produced (false negatives).
    pub missing: Vec<String>,
    /// Produced but not expected (false positives).
    pub unexpected: Vec<String>,
}

impl SetComparison {
    fn compare(expected: BTreeSet<String>, actual: BTreeSet<String>) -> Self {
        Self {
            expected: expected.len(),
            actual: actual.len(),
            true_positives: expected.intersection(&actual).count(),
            missing: expected.difference(&actual).cloned().collect(),
            unexpected: actual.difference(&expected).cloned().collect(),
        }
    }

    /// `None` when nothing was produced.
    pub fn precision(&self) -> Option<f64> {
        (self.actual > 0).then(|| self.true_positives as f64 / self.actual as f64)
    }

    /// `None` when nothing was expected.
    pub fn recall(&self) -> Option<f64> {
        (self.expected > 0).then(|| self.true_positives as f64 / self.expected as f64)
    }

    pub fn is_exact(&self) -> bool {
        self.missing.is_empty() && self.unexpected.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Evaluation {
    pub calls: SetComparison,
    pub implements: SetComparison,
    pub ambiguous: SetComparison,
    pub unresolved: SetComparison,
}

impl Evaluation {
    pub fn is_exact(&self) -> bool {
        self.calls.is_exact()
            && self.implements.is_exact()
            && self.ambiguous.is_exact()
            && self.unresolved.is_exact()
    }
}

pub fn evaluate(resolution: &Resolution, truth: &GroundTruth) -> Evaluation {
    let edges = |kind: EdgeKind| -> BTreeSet<String> {
        resolution
            .edges
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| pair(e.from.as_str(), e.to.as_str()))
            .collect()
    };
    let pairs = |list: &[(String, String)]| -> BTreeSet<String> {
        list.iter().map(|(a, b)| pair(a, b)).collect()
    };
    let sites = |list: &[ExpectedSite]| -> BTreeSet<String> {
        list.iter()
            .map(|s| site(&s.caller, &s.callee, &s.reason))
            .collect()
    };

    Evaluation {
        calls: SetComparison::compare(pairs(&truth.calls), edges(EdgeKind::Calls)),
        implements: SetComparison::compare(pairs(&truth.implements), edges(EdgeKind::Implements)),
        ambiguous: SetComparison::compare(
            sites(&truth.ambiguous),
            resolution
                .ambiguous_calls
                .iter()
                .map(|c| site(c.caller.as_str(), &c.callee, c.reason.as_str()))
                .collect(),
        ),
        unresolved: SetComparison::compare(
            sites(&truth.unresolved),
            resolution
                .unresolved_calls
                .iter()
                .map(|c| site(c.caller.as_str(), &c.callee, c.reason.as_str()))
                .collect(),
        ),
    }
}

fn pair(from: &str, to: &str) -> String {
    format!("{from} -> {to}")
}

fn site(caller: &str, callee: &str, reason: &str) -> String {
    format!("{caller} | {callee} | {reason}")
}
