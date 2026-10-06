//! The facts an explanation may use, gathered deterministically from the
//! code graph. Every fact is one relationship found by static analysis
//! (or one count from an impact report), with its file and lines, and is
//! numbered (`E1`, `E2`, ...) so that an explanation can cite it.

use std::collections::BTreeMap;

use codeatlas_analyzer::graph::impact::{
    impact_of_symbol, AffectedSymbol, Confidence, DependencyKind, EvidenceStep, ImpactError,
    ImpactOptions, ImpactReport,
};
use codeatlas_analyzer::graph::CodeGraph;
use codeatlas_analyzer::model::SymbolId;
use serde::{Deserialize, Serialize};

/// Chains included in an impact summary.
const MAX_SUMMARY_CHAINS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactKind {
    Calls,
    DispatchesTo,
    Implements,
    /// An ambiguous call: one of several possible targets.
    MayCall,
    /// A count from the impact report.
    Summary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    /// `E1`, `E2`, ...
    pub id: String,
    pub kind: FactKind,
    /// The dependent side, for relationships.
    pub source: Option<SymbolId>,
    /// The side closer to the change, for relationships.
    pub target: Option<SymbolId>,
    pub file: Option<String>,
    pub lines: Vec<u32>,
    /// The fact as one sentence, with code in backticks.
    pub text: String,
}

/// A symbol named in the evidence, with the names it may be called by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedSymbol {
    pub id: SymbolId,
    pub qualified_name: String,
    /// `Type::method` for methods, the name otherwise.
    pub short_name: String,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    /// The facts answer the question.
    Sufficient,
    /// The answer depends on ambiguous calls.
    Possible,
    /// No chain of facts answers the question.
    Insufficient,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// The question, in words.
    pub question: String,
    pub status: EvidenceStatus,
    /// Why the evidence is insufficient (or only possible), if it is.
    pub reason: Option<String>,
    pub facts: Vec<Fact>,
    /// Chains as fact IDs, from the affected symbol to the change.
    pub chains: Vec<Vec<String>>,
    pub symbols: Vec<NamedSymbol>,
    pub max_depth: u32,
}

struct Builder<'g> {
    graph: &'g CodeGraph,
    facts: Vec<Fact>,
    by_step: BTreeMap<(String, String, String), String>,
    symbols: BTreeMap<SymbolId, NamedSymbol>,
}

impl<'g> Builder<'g> {
    fn new(graph: &'g CodeGraph) -> Self {
        Self {
            graph,
            facts: Vec::new(),
            by_step: BTreeMap::new(),
            symbols: BTreeMap::new(),
        }
    }

    fn name(&mut self, id: &SymbolId) -> String {
        if let Some(named) = self.symbols.get(id) {
            return named.short_name.clone();
        }
        let named = match self.graph.node(id) {
            Some(node) => {
                let s = self.graph.symbol(node);
                let short_name = match s.kind {
                    codeatlas_analyzer::model::SymbolKind::Method => {
                        let mut parts = s.qualified_name.rsplitn(3, "::");
                        let name = parts.next().unwrap_or_default();
                        match parts.next() {
                            Some(owner) => format!("{owner}::{name}"),
                            None => name.to_string(),
                        }
                    }
                    _ => s.name.clone(),
                };
                NamedSymbol {
                    id: id.clone(),
                    qualified_name: s.qualified_name.clone(),
                    short_name,
                    file: s.file.clone(),
                    line: s.start_line,
                }
            }
            None => {
                let text = id.as_str().split_once(':').map_or(id.as_str(), |(_, q)| q);
                NamedSymbol {
                    id: id.clone(),
                    qualified_name: text.to_string(),
                    short_name: text.rsplit("::").next().unwrap_or(text).to_string(),
                    file: String::new(),
                    line: 0,
                }
            }
        };
        let short = named.short_name.clone();
        self.symbols.insert(id.clone(), named);
        short
    }

    fn push(&mut self, mut fact: Fact) -> String {
        let id = format!("E{}", self.facts.len() + 1);
        fact.id = id.clone();
        self.facts.push(fact);
        id
    }

    /// The fact for one step of an evidence chain (shared between chains).
    fn step(&mut self, step: &EvidenceStep) -> String {
        let kind = match step.kind {
            DependencyKind::Calls => FactKind::Calls,
            DependencyKind::DispatchesTo => FactKind::DispatchesTo,
            DependencyKind::Implements => FactKind::Implements,
            DependencyKind::MayCall => FactKind::MayCall,
        };
        let key = (
            step.source.to_string(),
            step.target.to_string(),
            format!("{kind:?}"),
        );
        if let Some(id) = self.by_step.get(&key) {
            return id.clone();
        }
        let (source, target) = (self.name(&step.source), self.name(&step.target));
        let at = location(&step.file, &step.lines);
        let text = match kind {
            FactKind::Calls => format!("`{source}` calls `{target}` ({at})."),
            FactKind::DispatchesTo => format!(
                "Calls to the trait method `{source}` may dispatch to its implementation `{target}` ({at})."
            ),
            FactKind::Implements => {
                format!("`{source}` implements the trait method `{target}` ({at}).")
            }
            FactKind::MayCall => format!(
                "`{source}` has a call that static analysis could not pin down to one target; `{target}` is one of the candidates ({at})."
            ),
            FactKind::Summary => unreachable!(),
        };
        let id = self.push(Fact {
            id: String::new(),
            kind,
            source: Some(step.source.clone()),
            target: Some(step.target.clone()),
            file: Some(step.file.clone()),
            lines: step.lines.clone(),
            text,
        });
        self.by_step.insert(key, id.clone());
        id
    }

    fn chain(&mut self, affected: &AffectedSymbol) -> Vec<String> {
        affected.path.iter().map(|s| self.step(s)).collect()
    }

    fn summary(&mut self, text: String) -> String {
        self.push(Fact {
            id: String::new(),
            kind: FactKind::Summary,
            source: None,
            target: None,
            file: None,
            lines: Vec::new(),
            text,
        })
    }

    fn finish(
        self,
        question: String,
        status: EvidenceStatus,
        reason: Option<String>,
        chains: Vec<Vec<String>>,
        max_depth: u32,
    ) -> Evidence {
        Evidence {
            question,
            status,
            reason,
            facts: self.facts,
            chains,
            symbols: self.symbols.into_values().collect(),
            max_depth,
        }
    }
}

fn location(file: &str, lines: &[u32]) -> String {
    match lines {
        [] => file.to_string(),
        [line] => format!("{file}:{line}"),
        _ => format!(
            "{file}:{}",
            lines
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// "Why does changing `changed` affect `affected`?"
pub fn why(
    graph: &CodeGraph,
    changed: &SymbolId,
    affected: &SymbolId,
    options: ImpactOptions,
) -> Result<Evidence, ImpactError> {
    if graph.node(affected).is_none() {
        return Err(ImpactError::UnknownSymbol(affected.to_string()));
    }
    let mut b = Builder::new(graph);
    let (changed_name, affected_name) = (b.name(changed), b.name(affected));
    let question = format!("Why can changing `{changed_name}` affect `{affected_name}`?");

    let certain = impact_of_symbol(
        graph,
        changed,
        ImpactOptions {
            include_ambiguous: false,
            ..options
        },
    )?;
    if let Some(found) = find(&certain, affected) {
        let chain = b.chain(found);
        return Ok(b.finish(
            question,
            EvidenceStatus::Sufficient,
            None,
            vec![chain],
            options.max_depth,
        ));
    }
    let possible = impact_of_symbol(
        graph,
        changed,
        ImpactOptions {
            include_ambiguous: true,
            ..options
        },
    )?;
    if let Some(found) = find(&possible, affected) {
        let chain = b.chain(found);
        return Ok(b.finish(
            question,
            EvidenceStatus::Possible,
            Some("the only chain found goes through calls static analysis could not resolve to one target, so the effect is possible, not certain".into()),
            vec![chain],
            options.max_depth,
        ));
    }

    // Nothing: say so, and whether the dependency runs the other way.
    let reverse = impact_of_symbol(
        graph,
        affected,
        ImpactOptions {
            include_ambiguous: false,
            ..options
        },
    )
    .ok()
    .and_then(|r| find(&r, changed).cloned());
    let (reason, chains) = match reverse {
        Some(found) => (
            format!(
                "no chain of relationships leads from `{affected_name}` to `{changed_name}` within depth {}; the dependency runs the other way: `{changed_name}` depends on `{affected_name}`",
                options.max_depth
            ),
            vec![b.chain(&found)],
        ),
        None => (
            format!(
                "no chain of calls, trait dispatch or implementations leads from `{affected_name}` to `{changed_name}` within depth {}, including through ambiguous calls",
                options.max_depth
            ),
            Vec::new(),
        ),
    };
    Ok(b.finish(
        question,
        EvidenceStatus::Insufficient,
        Some(reason),
        chains,
        options.max_depth,
    ))
}

fn find<'r>(report: &'r ImpactReport, id: &SymbolId) -> Option<&'r AffectedSymbol> {
    report.affected.iter().find(|a| &a.symbol.id == id)
}

/// "What can changing `changed` affect?" Counts and the most telling
/// chains: direct dependents first, then tests.
pub fn impact(
    graph: &CodeGraph,
    changed: &SymbolId,
    options: ImpactOptions,
) -> Result<Evidence, ImpactError> {
    let report = impact_of_symbol(graph, changed, options)?;
    let mut b = Builder::new(graph);
    let changed_name = b.name(changed);
    let question = format!("What can changing `{changed_name}` affect?");
    let certain: Vec<&AffectedSymbol> = report.certain().collect();
    if certain.is_empty() {
        let reason = format!(
            "nothing reaches `{changed_name}` through resolved calls, trait dispatch or implementations within depth {}",
            options.max_depth
        );
        return Ok(b.finish(
            question,
            EvidenceStatus::Insufficient,
            Some(reason),
            Vec::new(),
            options.max_depth,
        ));
    }
    let direct = report.direct().count();
    b.summary(format!(
        "{} can be affected: {} `{changed_name}` directly and {} it through other code; they are in {} and {}.",
        count(certain.len(), "symbol", "symbols"),
        count(direct, "calls", "call"),
        count(certain.len() - direct, "reaches", "reach"),
        count(report.files.len(), "file", "files"),
        count(report.modules.len(), "module", "modules")
    ));
    b.summary(format!(
        "{} `{changed_name}`; the blast-radius score is {:.1} of 100 ({:?}), a measure of how much of the graph is reached, not of how likely a breakage is.",
        match report.tests.len() {
            1 => "1 test reaches".to_string(),
            n => format!("{n} tests reach"),
        },
        report.score.total,
        report.score.level
    ));
    let mut picked: Vec<&AffectedSymbol> =
        certain.iter().copied().filter(|a| a.depth == 1).collect();
    picked.extend(
        certain
            .iter()
            .copied()
            .filter(|a| a.depth > 1 && a.symbol.is_test),
    );
    picked.extend(
        certain
            .iter()
            .copied()
            .filter(|a| a.depth > 1 && !a.symbol.is_test),
    );
    picked.truncate(MAX_SUMMARY_CHAINS);
    let chains = picked.iter().map(|a| b.chain(a)).collect();
    let possible = report
        .affected
        .iter()
        .filter(|a| a.confidence == Confidence::Possible)
        .count();
    let reason = (possible > 0).then(|| {
        format!(
            "{possible} more symbols are reached only through ambiguous calls and are not included"
        )
    });
    Ok(b.finish(
        question,
        EvidenceStatus::Sufficient,
        reason,
        chains,
        options.max_depth,
    ))
}

/// `1 file`, `2 files`.
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
