//! A deterministic explanation built from the evidence alone. It is used
//! when no model is configured or reachable, when the model's answer is
//! rejected, and whenever the evidence does not answer the question.

use crate::evidence::{Evidence, EvidenceStatus, Fact};

fn sentence(fact: &Fact) -> String {
    let text = fact.text.trim_end_matches('.');
    format!("{text} [{}].", fact.id)
}

fn facts<'e>(evidence: &'e Evidence, ids: &[String]) -> Vec<&'e Fact> {
    ids.iter()
        .filter_map(|id| evidence.facts.iter().find(|f| &f.id == id))
        .collect()
}

pub fn render(evidence: &Evidence) -> String {
    let mut out = Vec::new();
    match evidence.status {
        EvidenceStatus::Insufficient => {
            out.push(format!(
                "CodeAtlas has no evidence for this: {}.",
                evidence.reason.as_deref().unwrap_or("no chain was found")
            ));
            for chain in &evidence.chains {
                for fact in facts(evidence, chain) {
                    out.push(sentence(fact));
                }
            }
        }
        _ if evidence.chains.len() == 1 && !evidence.facts.iter().any(is_summary) => {
            let chain = &evidence.chains[0];
            let steps = facts(evidence, chain);
            let lead = if evidence.status == EvidenceStatus::Possible {
                "Possibly, through a chain that includes an unresolved call"
            } else {
                "Through a chain of resolved relationships"
            };
            out.push(format!(
                "{lead} ({} {}):",
                steps.len(),
                if steps.len() == 1 { "step" } else { "steps" }
            ));
            for fact in steps {
                out.push(sentence(fact));
            }
            if let Some(reason) = &evidence.reason {
                out.push(format!("Note: {reason}."));
            }
        }
        _ => {
            for fact in evidence.facts.iter().filter(|f| is_summary(f)) {
                out.push(sentence(fact));
            }
            if !evidence.chains.is_empty() {
                out.push("Examples, each from the affected symbol to the change:".into());
                for chain in &evidence.chains {
                    let steps = facts(evidence, chain);
                    let cites: Vec<&str> = steps.iter().map(|f| f.id.as_str()).collect();
                    let text: Vec<String> = steps
                        .iter()
                        .map(|f| f.text.trim_end_matches('.').to_string())
                        .collect();
                    out.push(format!("- {} [{}].", text.join("; "), cites.join(", ")));
                }
            }
            if let Some(reason) = &evidence.reason {
                out.push(format!("Note: {reason}."));
            }
        }
    }
    out.join("\n")
}

fn is_summary(fact: &Fact) -> bool {
    fact.kind == crate::evidence::FactKind::Summary
}
