//! The request sent to the model: the question and the numbered facts,
//! nothing else from the repository.

use serde::Serialize;

use crate::evidence::{Evidence, EvidenceStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub role: &'static str,
    pub content: String,
}

pub const SYSTEM: &str = "You explain the results of a static code-analysis tool to a developer. \
You receive a question and numbered facts that the tool found in the code. \
Answer the question in two to five plain sentences, using only the facts.\n\
Rules:\n\
1. Cite the facts behind every statement, like [E1] or [E1, E2].\n\
2. Mention only functions, methods, types, modules and files that appear in the facts, \
written in backticks exactly as in the facts.\n\
3. Do not add relationships, causes or behaviour that the facts do not state, \
and do not guess what the code does internally.\n\
4. If a fact says a call is ambiguous or an effect is only possible, say so.\n\
5. Do not use headings or lists.";

pub fn messages(evidence: &Evidence) -> Vec<Message> {
    let mut user = format!("Question: {}\n\nFacts:\n", evidence.question);
    for fact in &evidence.facts {
        user.push_str(&format!("[{}] {}\n", fact.id, fact.text));
    }
    if !evidence.chains.is_empty() {
        user.push_str("\nChains, from the affected symbol to the change:\n");
        for chain in &evidence.chains {
            user.push_str(&chain.join(" -> "));
            user.push('\n');
        }
    }
    if evidence.status == EvidenceStatus::Possible {
        user.push_str(
            "\nThe chain depends on an ambiguous call: the effect is possible, not certain.\n",
        );
    }
    if let Some(reason) = &evidence.reason {
        user.push_str(&format!("\nNote from the tool: {reason}.\n"));
    }
    user.push_str("\nAnswer:");
    vec![
        Message {
            role: "system",
            content: SYSTEM.to_string(),
        },
        Message {
            role: "user",
            content: user,
        },
    ]
}
