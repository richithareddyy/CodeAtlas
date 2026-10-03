//! Text rendering for impact reports and architecture analyses.

use std::fmt::Write as _;

use clap::ValueEnum;
use codeatlas_analyzer::graph::architecture::{Cycle, Hotspot, Layer, Level};
use codeatlas_analyzer::graph::impact::{
    AffectedSymbol, Confidence, DependencyKind, EvidenceStep, ImpactReport,
};

#[derive(Clone, Copy, ValueEnum)]
pub enum LevelArg {
    Function,
    File,
    Module,
}

impl From<LevelArg> for Level {
    fn from(level: LevelArg) -> Self {
        match level {
            LevelArg::Function => Level::Function,
            LevelArg::File => Level::File,
            LevelArg::Module => Level::Module,
        }
    }
}

/// `fn:shop::payments::authorize` → `shop::payments::authorize`.
fn short(id: &str) -> &str {
    id.split_once(':').map_or(id, |(_, rest)| rest)
}

fn step_text(step: &EvidenceStep) -> String {
    let (source, target) = (short(step.source.as_str()), short(step.target.as_str()));
    let lines = step
        .lines
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let fact = match step.kind {
        DependencyKind::Calls => format!("{source} calls {target}"),
        DependencyKind::DispatchesTo => format!("calls to {source} may dispatch to {target}"),
        DependencyKind::Implements => format!("{source} implements {target}"),
        DependencyKind::MayCall => format!("{source} may call {target} (ambiguous call)"),
    };
    format!("{fact}  ({}:{lines})", step.file)
}

fn affected_lines(out: &mut String, list: &[&AffectedSymbol]) {
    for a in list {
        let test = if a.symbol.is_test { "  [test]" } else { "" };
        let _ = writeln!(
            out,
            "  {:>2}  {:<70} {}:{}{test}",
            a.depth, a.symbol.id, a.symbol.file, a.symbol.line
        );
        for step in &a.path {
            let _ = writeln!(out, "        {}", step_text(step));
        }
    }
}

pub fn impact(report: &ImpactReport, limit: usize) -> String {
    let mut out = String::new();
    let changed = &report.changed;
    match changed.as_slice() {
        [only] => {
            let _ = writeln!(out, "Impact of {} ({}:{})", only.id, only.file, only.line);
        }
        _ => {
            let _ = writeln!(out, "Impact of {} changed symbols", changed.len());
            for c in changed.iter().take(limit) {
                let _ = writeln!(out, "  {}  ({}:{})", c.id, c.file, c.line);
            }
        }
    }
    let certain: Vec<&AffectedSymbol> = report.certain().collect();
    let possible: Vec<&AffectedSymbol> = report
        .affected
        .iter()
        .filter(|a| a.confidence == Confidence::Possible)
        .collect();
    let _ = writeln!(
        out,
        "Affected    {} symbols ({} direct, {} indirect) in {} files and {} modules; {} tests",
        certain.len(),
        report.direct().count(),
        report.indirect().count(),
        report.files.len(),
        report.modules.len(),
        report.tests.len()
    );
    if report.include_ambiguous {
        let _ = writeln!(
            out,
            "Possible    {} more through ambiguous calls",
            possible.len()
        );
    }
    let _ = writeln!(
        out,
        "Score       {:.1} / 100 ({:?}): size of the affected graph, not a probability of breakage",
        report.score.total, report.score.level
    );
    for f in &report.score.factors {
        let _ = writeln!(
            out,
            "  {:<22} {:>5}  saturates at {:<4} weight {:.2}  -> {:>5.1}",
            f.name, f.value, f.saturation, f.weight, f.contribution
        );
    }
    if report.truncated {
        let _ = writeln!(
            out,
            "Note        traversal stopped at the node limit; results are partial"
        );
    }

    let _ = writeln!(
        out,
        "\nAffected symbols (depth, symbol, location, then why)"
    );
    if certain.is_empty() {
        let _ = writeln!(out, "  none within depth {}", report.max_depth);
    }
    affected_lines(&mut out, &certain[..certain.len().min(limit)]);
    if certain.len() > limit {
        let _ = writeln!(
            out,
            "  … {} more (use --limit or --json)",
            certain.len() - limit
        );
    }
    if !possible.is_empty() {
        let _ = writeln!(out, "\nPossibly affected (through ambiguous calls)");
        affected_lines(&mut out, &possible[..possible.len().min(limit)]);
    }

    let _ = writeln!(out, "\nFiles");
    for f in report.files.iter().take(limit) {
        let _ = writeln!(out, "  {:>4}  {}", f.symbols, f.name);
    }
    if !report.tests.is_empty() {
        let _ = writeln!(out, "\nTests to run");
        for t in report.tests.iter().take(limit) {
            let _ = writeln!(out, "  {t}");
        }
    }
    out.trim_end().to_string()
}

pub fn cycles(cycles: &[Cycle], level: Level, limit: usize) -> String {
    let mut out = format!("{} {level:?}-level cycles\n", cycles.len());
    for (i, cycle) in cycles.iter().take(limit).enumerate() {
        let count = cycle.members.len();
        let noun = if count == 1 { "member" } else { "members" };
        let _ = writeln!(
            out,
            "\n{}. {count} {noun}: {}",
            i + 1,
            cycle.members.join(", ")
        );
        for hop in &cycle.hops {
            let _ = writeln!(
                out,
                "   {} -> {}  ({} edge{}: {})",
                hop.from,
                hop.to,
                hop.weight,
                if hop.weight == 1 { "" } else { "s" },
                hop.via.join(", ")
            );
            for e in &hop.evidence {
                let _ = writeln!(
                    out,
                    "       {:<10} {} -> {}  {}:{}",
                    e.relation, e.from, e.to, e.file, e.line
                );
            }
        }
    }
    if cycles.len() > limit {
        let _ = writeln!(out, "\n… {} more", cycles.len() - limit);
    }
    out.trim_end().to_string()
}

pub fn hotspots(spots: &[Hotspot]) -> String {
    let mut out = format!(
        "{:>12}  {:>6}  {:>7}  name\n",
        "betweenness", "fan-in", "fan-out"
    );
    for s in spots {
        let _ = writeln!(
            out,
            "{:>12.1}  {:>6}  {:>7}  {}",
            s.betweenness, s.fan_in, s.fan_out, s.name
        );
    }
    out.trim_end().to_string()
}

pub fn layers(layers: &[Layer]) -> String {
    let mut out = String::from("Layer 0 depends on nothing at this level; each layer depends only on lower ones (cycle members share a layer).\n");
    for layer in layers {
        let _ = writeln!(out, "\n{}:", layer.layer);
        for m in &layer.members {
            let _ = writeln!(out, "  {m}");
        }
    }
    out.trim_end().to_string()
}
