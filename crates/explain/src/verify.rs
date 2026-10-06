//! Checks a model's answer against the evidence before anyone sees it.
//!
//! An answer is rejected when it cites a fact that does not exist, cites
//! nothing, or states a relationship involving code that is not in the
//! evidence. These are the ways a model invents dependencies. Weaker
//! problems (an uncited sentence about a relationship, an unknown name in
//! passing) are reported as warnings.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::evidence::Evidence;

/// Words that make a sentence a statement about a relationship.
const RELATION_WORDS: &[&str] = &[
    "call",
    "depend",
    "dispatch",
    "implement",
    "reach",
    "affect",
    "invoke",
    "use",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    pub accepted: bool,
    /// Fact IDs the answer cites.
    pub cited: Vec<String>,
    /// Reasons for rejection.
    pub problems: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn verify(text: &str, evidence: &Evidence) -> Verification {
    let known: BTreeSet<&str> = evidence.facts.iter().map(|f| f.id.as_str()).collect();
    let names = Names::new(evidence);
    let mut cited = BTreeSet::new();
    let mut problems = Vec::new();
    let mut warnings = Vec::new();

    if text.trim().is_empty() {
        problems.push("the answer is empty".to_string());
    }
    for sentence in sentences(text) {
        let citations = citations(sentence);
        for id in &citations {
            if known.contains(id.as_str()) {
                cited.insert(id.clone());
            } else {
                problems.push(format!("cites {id}, which is not one of the facts"));
            }
        }
        let relational = {
            let lower = sentence.to_lowercase();
            RELATION_WORDS.iter().any(|w| lower.contains(w))
        };
        for name in code_names(sentence) {
            if names.known(&name) {
                continue;
            }
            if relational {
                problems.push(format!(
                    "states a relationship involving `{name}`, which is not in the evidence"
                ));
            } else {
                warnings.push(format!("mentions `{name}`, which is not in the evidence"));
            }
        }
        if relational && citations.is_empty() {
            warnings.push(format!("no citation for: \"{}\"", sentence.trim()));
        }
    }
    if cited.is_empty() && !evidence.facts.is_empty() && problems.is_empty() {
        problems.push("cites none of the facts".to_string());
    }
    problems.dedup();
    warnings.dedup();
    Verification {
        accepted: problems.is_empty(),
        cited: cited.into_iter().collect(),
        problems,
        warnings,
    }
}

/// Every name the evidence allows: qualified names, short names, bare
/// names, files.
struct Names {
    exact: BTreeSet<String>,
    qualified: Vec<String>,
}

impl Names {
    fn new(evidence: &Evidence) -> Self {
        let mut exact = BTreeSet::new();
        let mut qualified = Vec::new();
        for s in &evidence.symbols {
            exact.insert(s.short_name.clone());
            exact.insert(s.qualified_name.clone());
            if let Some(bare) = s.qualified_name.rsplit("::").next() {
                exact.insert(bare.to_string());
            }
            exact.insert(s.file.clone());
            qualified.push(s.qualified_name.clone());
        }
        for f in &evidence.facts {
            if let Some(file) = &f.file {
                exact.insert(file.clone());
            }
        }
        exact.remove("");
        Self { exact, qualified }
    }

    fn known(&self, name: &str) -> bool {
        let name = name.trim_end_matches("()");
        // `file.rs:12` refers to a file.
        let file = name.split(':').next().unwrap_or(name);
        self.exact.contains(name)
            || self.exact.contains(file)
            || self
                .qualified
                .iter()
                .any(|q| q.ends_with(&format!("::{name}")))
    }
}

/// Splits at `.`, `!` or `?` followed by whitespace or the end, so that
/// `src/lib.rs:5` stays whole.
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (i, &(at, c)) in chars.iter().enumerate() {
        let next = chars.get(i + 1).map(|&(_, n)| n);
        if matches!(c, '.' | '!' | '?' | '\n') && next.is_none_or(char::is_whitespace) {
            let end = at + c.len_utf8();
            if !text[start..end].trim().is_empty() {
                out.push(&text[start..end]);
            }
            start = end;
        }
    }
    if !text[start..].trim().is_empty() {
        out.push(&text[start..]);
    }
    out
}

/// `[E1]`, `[E1, E2]`, `[E3][E4]`.
fn citations(sentence: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = sentence;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']') else {
            break;
        };
        let inner = &rest[open + 1..open + close];
        let ids: Vec<&str> = inner
            .split([',', ' ', ';'])
            .filter(|t| !t.is_empty())
            .collect();
        if !ids.is_empty()
            && ids.iter().all(|t| {
                t.len() > 1 && t.starts_with('E') && t[1..].chars().all(|c| c.is_ascii_digit())
            })
        {
            out.extend(ids.iter().map(|t| t.to_string()));
        }
        rest = &rest[open + close + 1..];
    }
    out
}

/// Code in backticks, and bare paths such as `a::b` outside them.
fn code_names(sentence: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut outside = String::new();
    for (i, part) in sentence.split('`').enumerate() {
        if i % 2 == 1 {
            if !part.trim().is_empty() {
                out.push(part.trim().to_string());
            }
        } else {
            outside.push_str(part);
            outside.push(' ');
        }
    }
    for word in outside.split_whitespace() {
        let word = word.trim_matches(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'));
        if word.contains("::") && !word.starts_with("::") {
            out.push(word.trim_end_matches(':').to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_sentences_but_keeps_file_locations() {
        let found: Vec<&str> =
            sentences("`a` calls `b` (src/lib.rs:5) [E1]. So `b` matters [E1]!\nDone")
                .into_iter()
                .map(str::trim)
                .collect();
        assert_eq!(
            found,
            [
                "`a` calls `b` (src/lib.rs:5) [E1].",
                "So `b` matters [E1]!",
                "Done"
            ]
        );
    }

    #[test]
    fn reads_citations() {
        assert_eq!(
            citations("x [E1, E2] y [E10][E3]"),
            ["E1", "E2", "E10", "E3"]
        );
        assert!(citations("a [see above] b [1]").is_empty());
    }

    #[test]
    fn finds_code_names() {
        assert_eq!(
            code_names("`checkout` calls payments::authorize() and `Gateway::charge`."),
            ["checkout", "Gateway::charge", "payments::authorize"]
        );
    }
}
