//! Text and Markdown rendering of a [`DiffReport`].

use std::fmt::Write as _;

use codeatlas_analyzer::diff::{ChangeKind, DiffReport, DownstreamSymbol, Side, SymbolChange};
use codeatlas_analyzer::git::FileStatus;
use codeatlas_analyzer::graph::impact::Confidence;
use codeatlas_analyzer::model::SymbolKind;

use crate::views::{short, step_text};

fn revisions(report: &DiffReport) -> String {
    let sha = |sha: &Option<String>| match sha {
        Some(sha) => sha[..sha.len().min(10)].to_string(),
        None => "working tree".to_string(),
    };
    format!(
        "{}..{}  ({}..{})",
        report.base.label,
        report.head.label,
        sha(&report.base.sha),
        sha(&report.head.sha)
    )
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `1 added, 2 removed` from non-zero counts, or `none`.
fn counts(parts: &[(u32, &str)]) -> String {
    let text: Vec<String> = parts
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, label)| format!("{n} {label}"))
        .collect();
    if text.is_empty() {
        "none".to_string()
    } else {
        text.join(", ")
    }
}

fn kind_label(kind: SymbolKind) -> &'static str {
    kind.tag()
}

fn lines_text(change: &SymbolChange) -> String {
    let ranges: Vec<String> = change
        .lines
        .iter()
        .map(|l| {
            if l.start == l.end {
                l.start.to_string()
            } else {
                format!("{}-{}", l.start, l.end)
            }
        })
        .collect();
    if ranges.is_empty() {
        format!("{}:{}", change.symbol.file, change.symbol.line)
    } else {
        format!("{}:{}", change.symbol.file, ranges.join(","))
    }
}

fn certain(report: &DiffReport) -> Vec<&DownstreamSymbol> {
    report
        .impact
        .downstream
        .iter()
        .filter(|d| d.affected.confidence == Confidence::Certain)
        .collect()
}

fn possible(report: &DiffReport) -> Vec<&DownstreamSymbol> {
    report
        .impact
        .downstream
        .iter()
        .filter(|d| d.affected.confidence == Confidence::Possible)
        .collect()
}

pub fn text(report: &DiffReport, limit: usize) -> String {
    let s = &report.summary;
    let mut out = String::new();
    let _ = writeln!(out, "PR impact   {}", revisions(report));
    let _ = writeln!(
        out,
        "Files       {} changed: {}",
        s.files_changed,
        counts(&[
            (s.files_added, "added"),
            (s.files_removed, "removed"),
            (s.files_modified, "modified"),
            (s.files_renamed, "renamed"),
        ])
    );
    let _ = writeln!(
        out,
        "Functions   {}",
        counts(&[
            (s.functions_modified, "modified"),
            (s.functions_added, "added"),
            (s.functions_removed, "removed"),
        ])
    );
    let _ = writeln!(out, "Signatures  {} changed", s.signatures_changed);
    let _ = writeln!(
        out,
        "Types       {}",
        counts(&[
            (s.types_modified, "modified"),
            (s.types_added, "added"),
            (s.types_removed, "removed"),
        ])
    );
    let _ = writeln!(
        out,
        "Tests       {}",
        counts(&[
            (s.tests_modified, "modified"),
            (s.tests_added, "added"),
            (s.tests_removed, "removed"),
        ])
    );
    let _ = writeln!(
        out,
        "Also        {} moved, {} with whitespace or comment changes only",
        plural(s.moved, "symbol", "symbols"),
        s.cosmetic
    );
    let _ = writeln!(
        out,
        "\nPotential impact (resolved calls and trait dispatch, depth <= {})",
        report.impact.max_depth
    );
    let _ = writeln!(
        out,
        "  {} in {} and {}; {} to run",
        plural(
            s.downstream_symbols,
            "downstream symbol",
            "downstream symbols"
        ),
        plural(s.affected_modules, "module", "modules"),
        plural(s.affected_files, "file", "files"),
        plural(s.affected_tests, "test", "tests"),
    );
    if report.impact.include_ambiguous {
        let _ = writeln!(
            out,
            "  {} more possibly affected through ambiguous calls",
            s.possible_symbols
        );
    }
    if report.impact.truncated {
        let _ = writeln!(
            out,
            "  Note: traversal stopped at the node limit; results are partial"
        );
    }

    let _ = writeln!(out, "\nChanged symbols");
    if report.symbols.is_empty() {
        let _ = writeln!(out, "  none");
    }
    for c in report.symbols.iter().take(limit) {
        let (mark, note) = match c.change {
            ChangeKind::Modified => ("~", String::new()),
            ChangeKind::Added => ("+", String::new()),
            ChangeKind::Removed => ("-", "  (base revision)".to_string()),
            ChangeKind::Moved => (
                ">",
                format!(
                    "  moved from {}",
                    c.previous.as_ref().map_or("?", |p| short(p.id.as_str()))
                ),
            ),
        };
        let _ = writeln!(
            out,
            "  {mark} {:<6} {:<60} {}{note}",
            kind_label(c.symbol.kind),
            c.symbol.qualified_name,
            lines_text(c)
        );
        if let Some(sig) = &c.signature {
            let _ = writeln!(out, "      signature before: {}", sig.before);
            let _ = writeln!(out, "      signature after:  {}", sig.after);
        }
    }
    if report.symbols.len() > limit {
        let _ = writeln!(
            out,
            "  … {} more (use --limit or --format json)",
            report.symbols.len() - limit
        );
    }
    if !report.cosmetic.is_empty() {
        let _ = writeln!(
            out,
            "\nWhitespace or comment changes only (not treated as changes)"
        );
        for c in report.cosmetic.iter().take(limit) {
            let _ = writeln!(
                out,
                "    {:<6} {:<60} {}:{}",
                kind_label(c.kind),
                c.qualified_name,
                c.file,
                c.line
            );
        }
    }

    let certain = certain(report);
    let _ = writeln!(
        out,
        "\nDownstream symbols (depth, symbol, location, then why)"
    );
    if certain.is_empty() {
        let _ = writeln!(out, "  none within depth {}", report.impact.max_depth);
    }
    downstream_lines(&mut out, &certain[..certain.len().min(limit)]);
    if certain.len() > limit {
        let _ = writeln!(
            out,
            "  … {} more (use --limit or --format json)",
            certain.len() - limit
        );
    }
    let possible = possible(report);
    if !possible.is_empty() {
        let _ = writeln!(out, "\nPossibly affected (through ambiguous calls)");
        downstream_lines(&mut out, &possible[..possible.len().min(limit)]);
    }
    if !report.impact.tests.is_empty() {
        let _ = writeln!(out, "\nTests to run");
        for t in report.impact.tests.iter().take(limit) {
            let _ = writeln!(out, "  {}", short(t.as_str()));
        }
    }

    let _ = writeln!(out, "\nFiles");
    for f in report.files.iter().take(limit) {
        let status = match f.status {
            FileStatus::Added => "A",
            FileStatus::Removed => "D",
            FileStatus::Modified => "M",
            FileStatus::Renamed => "R",
        };
        let path = match &f.old_path {
            Some(old) => format!("{old} -> {}", f.path),
            None => f.path.clone(),
        };
        let note = if f.rust { "" } else { "  (not analysed)" };
        let _ = writeln!(out, "  {status} {path}{note}");
    }
    if report.files.len() > limit {
        let _ = writeln!(out, "  … {} more", report.files.len() - limit);
    }
    let _ = writeln!(
        out,
        "\nAnalysis    base {:.0} ms, head {:.0} ms, total {:.0} ms",
        report.stats.base_analysis_ms, report.stats.head_analysis_ms, report.stats.total_ms
    );
    out.trim_end().to_string()
}

fn downstream_lines(out: &mut String, list: &[&DownstreamSymbol]) {
    for d in list {
        let a = &d.affected;
        let test = if a.symbol.is_test { "  [test]" } else { "" };
        let revision = match d.revision {
            Side::Head => "",
            Side::Base => "  [via removed code; lines are from the base revision]",
        };
        let _ = writeln!(
            out,
            "  {:>2}  {:<64} {}:{}{test}{revision}",
            a.depth, a.symbol.qualified_name, a.symbol.file, a.symbol.line
        );
        for step in &a.path {
            let _ = writeln!(out, "        {}", step_text(step));
        }
    }
}

/// Plain text that GitHub would otherwise read as HTML tags
/// (`<StripeGateway as Gateway>::charge`).
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Code span safe inside a Markdown table cell.
fn code(text: &str) -> String {
    format!("`{}`", text.replace('|', "\\|").replace('`', "'"))
}

/// A Markdown summary suitable for a pull-request comment.
pub fn markdown(report: &DiffReport, limit: usize) -> String {
    let s = &report.summary;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "## Change impact: {} → {}\n",
        code(&report.base.label),
        code(&report.head.label)
    );
    let _ = writeln!(out, "| | |\n|---|---|");
    let rows = [
        (
            "Files changed",
            format!(
                "{} ({})",
                s.files_changed,
                counts(&[
                    (s.files_added, "added"),
                    (s.files_removed, "removed"),
                    (s.files_modified, "modified"),
                    (s.files_renamed, "renamed"),
                ])
            ),
        ),
        (
            "Functions",
            counts(&[
                (s.functions_modified, "modified"),
                (s.functions_added, "added"),
                (s.functions_removed, "removed"),
            ]),
        ),
        ("Signatures changed", s.signatures_changed.to_string()),
        (
            "Types",
            counts(&[
                (s.types_modified, "modified"),
                (s.types_added, "added"),
                (s.types_removed, "removed"),
            ]),
        ),
        (
            "Tests changed",
            counts(&[
                (s.tests_modified, "modified"),
                (s.tests_added, "added"),
                (s.tests_removed, "removed"),
            ]),
        ),
        (
            "Downstream symbols",
            format!(
                "{} in {}",
                s.downstream_symbols,
                plural(s.affected_modules, "module", "modules")
            ),
        ),
        ("Tests to run", s.affected_tests.to_string()),
    ];
    for (label, value) in rows {
        let _ = writeln!(out, "| {label} | {value} |");
    }

    if !report.symbols.is_empty() {
        let _ = writeln!(out, "\n### Changed symbols\n");
        let _ = writeln!(out, "| Change | Symbol | Lines |\n|---|---|---|");
        for c in report.symbols.iter().take(limit) {
            let change = match c.change {
                ChangeKind::Modified if c.signature.is_some() => "signature changed",
                ChangeKind::Modified => "modified",
                ChangeKind::Added => "added",
                ChangeKind::Removed => "removed",
                ChangeKind::Moved => "moved",
            };
            let mut symbol = format!(
                "{} {}",
                kind_label(c.symbol.kind),
                code(&c.symbol.qualified_name)
            );
            if c.change == ChangeKind::Moved {
                if let Some(previous) = &c.previous {
                    let _ = write!(symbol, " from {}", code(&previous.qualified_name));
                }
            }
            let _ = writeln!(out, "| {change} | {symbol} | {} |", code(&lines_text(c)));
        }
        if report.symbols.len() > limit {
            let _ = writeln!(
                out,
                "\n{} more changed symbols not shown.",
                report.symbols.len() - limit
            );
        }
        let signatures: Vec<&SymbolChange> = report
            .symbols
            .iter()
            .filter(|c| c.signature.is_some())
            .collect();
        if !signatures.is_empty() {
            let _ = writeln!(out, "\n#### Signature changes\n");
            for c in signatures.iter().take(limit) {
                let sig = c.signature.as_ref().expect("filtered");
                let _ = writeln!(
                    out,
                    "{}\n```diff\n- {}\n+ {}\n```",
                    code(&c.symbol.qualified_name),
                    sig.before,
                    sig.after
                );
            }
        }
    }

    let certain = certain(report);
    let _ = writeln!(out, "\n### Downstream symbols\n");
    if certain.is_empty() {
        let _ = writeln!(out, "None found within depth {}.", report.impact.max_depth);
    } else {
        let _ = writeln!(
            out,
            "<details><summary>{} symbols that depend on the changes, with the call chain for each</summary>\n",
            certain.len()
        );
        for d in certain.iter().take(limit) {
            let a = &d.affected;
            let test = if a.symbol.is_test { " (test)" } else { "" };
            let revision = if d.revision == Side::Base {
                " (through removed code; lines from the base revision)"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "- {}{test}, depth {}, {}{revision}",
                code(&a.symbol.qualified_name),
                a.depth,
                code(&format!("{}:{}", a.symbol.file, a.symbol.line))
            );
            for step in &a.path {
                let _ = writeln!(out, "  - {}", escape_html(&step_text(step)));
            }
        }
        if certain.len() > limit {
            let _ = writeln!(out, "\n{} more not shown.", certain.len() - limit);
        }
        let _ = writeln!(out, "\n</details>");
    }
    if !report.impact.tests.is_empty() {
        let _ = writeln!(out, "\n### Tests to run\n");
        for t in report.impact.tests.iter().take(limit) {
            let _ = writeln!(out, "- {}", code(short(t.as_str())));
        }
    }
    let _ = writeln!(
        out,
        "\n<sub>CodeAtlas static analysis of {}. Downstream symbols are reached through resolved calls and trait dispatch; this measures reach, not the likelihood of breakage.</sub>",
        code(&revisions(report))
    );
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::{code, escape_html};

    #[test]
    fn markdown_text_is_escaped() {
        assert_eq!(
            escape_html("<S as T>::m calls f && g"),
            "&lt;S as T&gt;::m calls f &amp;&amp; g"
        );
        assert_eq!(code("fn a(x: |u8| u8)"), "`fn a(x: \\|u8\\| u8)`");
    }
}
