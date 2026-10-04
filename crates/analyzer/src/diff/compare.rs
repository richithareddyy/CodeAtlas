//! Symbol-level comparison of two analyses of the same repository.
//!
//! Symbols are matched by ID. A symbol present on both sides is *modified*
//! when its fingerprint (tokens without whitespace and comments) differs;
//! when only formatting or comments changed inside it, it is *cosmetic*.
//! A removed and an added symbol of the same kind and name with the same
//! fingerprint are paired as *moved* (for example after a file rename),
//! provided the pairing is unambiguous.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::git::Hunk;
use crate::graph::impact::SymbolRef;
use crate::model::{Span, Symbol, SymbolId, SymbolKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Modified,
    Added,
    Removed,
    Moved,
}

/// 1-based, inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureChange {
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolChange {
    pub change: ChangeKind,
    /// The symbol in the head revision; in the base revision for removed
    /// symbols.
    pub symbol: SymbolRef,
    /// The symbol in the base revision, for modified and moved symbols.
    pub previous: Option<SymbolRef>,
    /// Set when a modified symbol's signature text changed.
    pub signature: Option<SignatureChange>,
    /// Evidence: the changed lines inside the symbol, in `symbol.file`. For
    /// modified symbols these come from the diff hunks (a pure deletion is
    /// shown as the line after which lines were removed); for added and
    /// removed symbols they are the symbol's whole span.
    pub lines: Vec<LineRange>,
}

#[derive(Debug, Default)]
pub struct Comparison {
    pub changes: Vec<SymbolChange>,
    /// Symbols touched by the diff whose tokens did not change.
    pub cosmetic: Vec<SymbolRef>,
}

impl From<&Symbol> for SymbolRef {
    fn from(s: &Symbol) -> Self {
        SymbolRef {
            id: s.id.clone(),
            kind: s.kind,
            qualified_name: s.qualified_name.clone(),
            file: s.file.clone(),
            line: s.span.start_line,
            is_test: s.is_test,
        }
    }
}

/// Compares the symbols of two revisions. `hunks` are keyed by head-side
/// path.
pub fn compare(
    base: &[&Symbol],
    head: &[&Symbol],
    hunks: &BTreeMap<String, Vec<Hunk>>,
) -> Comparison {
    let base_by_id: HashMap<&SymbolId, &Symbol> = base.iter().map(|s| (&s.id, *s)).collect();
    let head_by_id: HashMap<&SymbolId, &Symbol> = head.iter().map(|s| (&s.id, *s)).collect();
    let removed: Vec<&Symbol> = base
        .iter()
        .copied()
        .filter(|s| !head_by_id.contains_key(&s.id))
        .collect();
    let added: Vec<&Symbol> = head
        .iter()
        .copied()
        .filter(|s| !base_by_id.contains_key(&s.id))
        .collect();
    let moves = pair_moves(&removed, &added);
    let moved_from: HashSet<&SymbolId> = moves.iter().map(|(from, _)| &from.id).collect();
    let moved_to: HashSet<&SymbolId> = moves.iter().map(|(_, to)| &to.id).collect();

    // Child spans, so that a trait is not reported because one of its
    // methods changed.
    let mut children: HashMap<&SymbolId, Vec<Span>> = HashMap::new();
    for s in head {
        if let Some(parent) = &s.parent {
            if s.file == head_by_id.get(parent).map_or("", |p| p.file.as_str()) {
                children.entry(parent).or_default().push(s.span);
            }
        }
    }

    let mut out = Comparison::default();
    for s in head {
        let Some(before) = base_by_id.get(&s.id) else {
            continue;
        };
        if s.kind == SymbolKind::Module {
            continue;
        }
        let touched = touched_lines(hunks.get(&s.file), s.span);
        if s.fingerprint != before.fingerprint {
            let signature = match (&before.signature, &s.signature) {
                (Some(old), Some(new)) if old != new => Some(SignatureChange {
                    before: old.clone(),
                    after: new.clone(),
                }),
                _ => None,
            };
            out.changes.push(SymbolChange {
                change: ChangeKind::Modified,
                symbol: SymbolRef::from(*s),
                previous: Some(SymbolRef::from(*before)),
                signature,
                lines: touched,
            });
        } else if touched.iter().any(|range| {
            !children
                .get(&s.id)
                .is_some_and(|spans| covered(*range, spans))
        }) {
            out.cosmetic.push(SymbolRef::from(*s));
        }
    }
    for s in &added {
        if !moved_to.contains(&s.id) {
            out.changes.push(whole(ChangeKind::Added, s, None));
        }
    }
    for s in &removed {
        if !moved_from.contains(&s.id) {
            out.changes.push(whole(ChangeKind::Removed, s, None));
        }
    }
    for (from, to) in &moves {
        let mut change = whole(ChangeKind::Moved, to, Some(from));
        change.lines = Vec::new();
        out.changes.push(change);
    }

    out.changes.sort_by(|a, b| {
        (a.change, &a.symbol.file, a.symbol.line, &a.symbol.id).cmp(&(
            b.change,
            &b.symbol.file,
            b.symbol.line,
            &b.symbol.id,
        ))
    });
    out.cosmetic
        .sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    out
}

fn whole(change: ChangeKind, s: &Symbol, previous: Option<&Symbol>) -> SymbolChange {
    SymbolChange {
        change,
        symbol: SymbolRef::from(s),
        previous: previous.map(SymbolRef::from),
        signature: None,
        lines: vec![LineRange {
            start: s.span.start_line,
            end: s.span.end_line,
        }],
    }
}

/// Pairs removed and added symbols that are the same code under a new ID.
/// Modules have no fingerprint and are never paired.
fn pair_moves<'a>(removed: &[&'a Symbol], added: &[&'a Symbol]) -> Vec<(&'a Symbol, &'a Symbol)> {
    type Key<'k> = (SymbolKind, &'k str, &'k str);
    let key = |s: &'a Symbol| -> Option<Key<'a>> {
        Some((s.kind, s.name.as_str(), s.fingerprint.as_deref()?))
    };
    let mut groups: BTreeMap<Key<'a>, (Vec<&'a Symbol>, Vec<&'a Symbol>)> = BTreeMap::new();
    for s in removed {
        if let Some(k) = key(s) {
            groups.entry(k).or_default().0.push(s);
        }
    }
    for s in added {
        if let Some(k) = key(s) {
            groups.entry(k).or_default().1.push(s);
        }
    }
    groups
        .into_values()
        .filter_map(|(from, to)| match (from.as_slice(), to.as_slice()) {
            ([from], [to]) => Some((*from, *to)),
            _ => None,
        })
        .collect()
}

/// Lines of `span` touched by `hunks` (head side).
fn touched_lines(hunks: Option<&Vec<Hunk>>, span: Span) -> Vec<LineRange> {
    let Some(hunks) = hunks else {
        return Vec::new();
    };
    hunks
        .iter()
        .filter_map(|h| {
            let (start, end) = if h.new_lines == 0 {
                // Deleted lines sat between `new_start` and `new_start + 1`;
                // only a deletion strictly inside the span touches it.
                if h.new_start < span.start_line || h.new_start >= span.end_line {
                    return None;
                }
                (h.new_start, h.new_start)
            } else {
                (h.new_start, h.new_start + h.new_lines - 1)
            };
            let start = start.max(span.start_line);
            let end = end.min(span.end_line);
            (start <= end).then_some(LineRange { start, end })
        })
        .collect()
}

/// Whether every line of `range` lies inside one of `spans`.
fn covered(range: LineRange, spans: &[Span]) -> bool {
    (range.start..=range.end).all(|line| {
        spans
            .iter()
            .any(|s| s.start_line <= line && line <= s.end_line)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Visibility;

    fn sym(id: &str, file: &str, lines: (u32, u32), fp: &str, sig: &str) -> Symbol {
        let (tag, qualified) = id.split_once(':').unwrap();
        let kind = match tag {
            "mod" => SymbolKind::Module,
            "trait" => SymbolKind::Trait,
            "method" => SymbolKind::Method,
            _ => SymbolKind::Function,
        };
        Symbol {
            id: SymbolId::from_stored(id.to_string()),
            kind,
            name: qualified.rsplit("::").next().unwrap().to_string(),
            qualified_name: qualified.to_string(),
            file: file.to_string(),
            span: Span {
                start_line: lines.0,
                end_line: lines.1,
            },
            parent: None,
            visibility: Visibility::Public,
            signature: (!sig.is_empty()).then(|| sig.to_string()),
            is_test: false,
            cfg_test: false,
            return_type: None,
            type_params: Vec::new(),
            fingerprint: (kind != SymbolKind::Module).then(|| fp.to_string()),
        }
    }

    fn hunk(new_start: u32, new_lines: u32) -> Hunk {
        Hunk {
            old_start: new_start,
            old_lines: 1,
            new_start,
            new_lines,
        }
    }

    fn render(c: &Comparison) -> Vec<String> {
        let mut out: Vec<String> = c
            .changes
            .iter()
            .map(|c| {
                let from = c
                    .previous
                    .as_ref()
                    .filter(|p| p.id != c.symbol.id)
                    .map(|p| format!(" from {}", p.id))
                    .unwrap_or_default();
                let sig = if c.signature.is_some() { " [sig]" } else { "" };
                let lines: Vec<String> = c
                    .lines
                    .iter()
                    .map(|l| format!("{}-{}", l.start, l.end))
                    .collect();
                format!(
                    "{:?} {}{from}{sig} {}",
                    c.change,
                    c.symbol.id,
                    lines.join(",")
                )
            })
            .collect();
        out.extend(c.cosmetic.iter().map(|s| format!("Cosmetic {}", s.id)));
        out
    }

    #[test]
    fn classifies_modified_added_removed_moved_and_cosmetic_symbols() {
        let base = [
            sym("mod:app", "src/lib.rs", (1, 40), "", ""),
            sym("fn:app::pay", "src/lib.rs", (1, 5), "p1", "fn pay(a: u64)"),
            sym("fn:app::fee", "src/lib.rs", (7, 9), "f1", "fn fee()"),
            sym("fn:app::tidy", "src/lib.rs", (11, 14), "t1", "fn tidy()"),
            sym("fn:app::same", "src/lib.rs", (16, 18), "s1", "fn same()"),
            sym("fn:app::old::fmt", "src/old.rs", (1, 3), "m1", "fn fmt()"),
        ];
        let head = [
            sym("mod:app", "src/lib.rs", (1, 40), "", ""),
            sym(
                "fn:app::pay",
                "src/lib.rs",
                (1, 6),
                "p2",
                "fn pay(a: u64, c: &str)",
            ),
            sym("fn:app::tidy", "src/lib.rs", (8, 12), "t1", "fn tidy()"),
            sym("fn:app::same", "src/lib.rs", (14, 16), "s1", "fn same()"),
            sym(
                "fn:app::new_fn",
                "src/lib.rs",
                (18, 20),
                "n1",
                "fn new_fn()",
            ),
            sym(
                "fn:app::fmt_mod::fmt",
                "src/fmt_mod.rs",
                (1, 3),
                "m1",
                "fn fmt()",
            ),
        ];
        let hunks = BTreeMap::from([(
            "src/lib.rs".to_string(),
            // pay: lines 2-3 rewritten; fee removed (deletion after line 6);
            // tidy: a comment line added at 10; new_fn added at 18-20.
            vec![hunk(2, 2), hunk(6, 0), hunk(10, 1), hunk(18, 3)],
        )]);
        let base_refs: Vec<&Symbol> = base.iter().collect();
        let head_refs: Vec<&Symbol> = head.iter().collect();
        let result = compare(&base_refs, &head_refs, &hunks);
        assert_eq!(
            render(&result),
            [
                "Modified fn:app::pay [sig] 2-3",
                "Added fn:app::new_fn 18-20",
                "Removed fn:app::fee 7-9",
                "Moved fn:app::fmt_mod::fmt from fn:app::old::fmt ",
                "Cosmetic fn:app::tidy",
            ]
        );
    }

    #[test]
    fn deletions_touch_only_the_inside_of_a_span_and_children_are_excluded() {
        let span = Span {
            start_line: 10,
            end_line: 20,
        };
        // A deletion right after the last line is outside the symbol.
        assert!(touched_lines(Some(&vec![hunk(20, 0)]), span).is_empty());
        assert!(touched_lines(Some(&vec![hunk(9, 0)]), span).is_empty());
        assert_eq!(
            touched_lines(Some(&vec![hunk(12, 0), hunk(8, 4)]), span),
            [
                LineRange { start: 12, end: 12 },
                LineRange { start: 10, end: 11 }
            ]
        );

        // A trait whose method changed is neither modified nor cosmetic.
        let mut base_method = sym("method:app::T::m", "src/lib.rs", (2, 2), "m1", "fn m()");
        base_method.parent = Some(SymbolId::from_stored("trait:app::T".into()));
        let mut head_method = base_method.clone();
        head_method.fingerprint = Some("m2".into());
        let trait_symbol = sym("trait:app::T", "src/lib.rs", (1, 3), "t", "trait T");
        let hunks = BTreeMap::from([("src/lib.rs".to_string(), vec![hunk(2, 1)])]);
        let result = compare(
            &[&trait_symbol, &base_method],
            &[&trait_symbol, &head_method],
            &hunks,
        );
        assert_eq!(render(&result), ["Modified method:app::T::m 2-2"]);
    }

    #[test]
    fn ambiguous_moves_stay_added_and_removed() {
        let base = [
            sym("fn:app::a::new", "src/a.rs", (1, 1), "same", "fn new()"),
            sym("fn:app::b::new", "src/b.rs", (1, 1), "same", "fn new()"),
        ];
        let head = [
            sym("fn:app::c::new", "src/c.rs", (1, 1), "same", "fn new()"),
            sym("fn:app::d::new", "src/d.rs", (1, 1), "same", "fn new()"),
        ];
        let result = compare(
            &base.iter().collect::<Vec<_>>(),
            &head.iter().collect::<Vec<_>>(),
            &BTreeMap::new(),
        );
        let kinds: Vec<ChangeKind> = result.changes.iter().map(|c| c.change).collect();
        assert_eq!(
            kinds,
            [
                ChangeKind::Added,
                ChangeKind::Added,
                ChangeKind::Removed,
                ChangeKind::Removed
            ]
        );
    }
}
