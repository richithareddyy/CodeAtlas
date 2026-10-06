//! Optional explanations of CodeAtlas results in plain language.
//!
//! The analysis decides what is true; a language model only puts it into
//! words. [`evidence`] gathers numbered facts deterministically from the
//! code graph. When they answer the question, the facts (and nothing else
//! from the repository) are sent to a local model through Ollama, and
//! [`verify`] checks the answer: every citation must name a fact, and no
//! sentence may state a relationship involving code outside the evidence.
//! A rejected answer is replaced by the deterministic [`template`]
//! explanation, which is also used when no model is configured or
//! reachable, and always when the evidence is insufficient: then nothing is
//! sent to the model and the explanation says what is missing.

pub mod evidence;
pub mod ollama;
pub mod prompt;
pub mod template;
pub mod verify;

use serde::{Deserialize, Serialize};

pub use evidence::{Evidence, EvidenceStatus, Fact, FactKind};
pub use ollama::{ModelConfig, ModelError};
pub use verify::Verification;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSource {
    /// Written by the model and accepted by the verifier.
    Model,
    /// Built from the evidence without a model.
    Template,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    pub evidence: Evidence,
    pub text: String,
    pub source: TextSource,
    /// The model asked, if one was.
    pub model: Option<String>,
    /// The verifier's report on the model's answer, if there was one.
    pub verification: Option<Verification>,
    /// A model answer that was rejected, kept for transparency.
    pub rejected_text: Option<String>,
    /// What happened, for the reader (e.g. why no model was used).
    pub notes: Vec<String>,
}

/// Explains `evidence`, with the model in `model` when there is one.
pub async fn explain(evidence: Evidence, model: Option<&ModelConfig>) -> Explanation {
    let template = template::render(&evidence);
    let fallback = |evidence: Evidence, note: String, model: Option<String>| Explanation {
        text: template.clone(),
        evidence,
        source: TextSource::Template,
        model,
        verification: None,
        rejected_text: None,
        notes: vec![note],
    };
    if evidence.status == EvidenceStatus::Insufficient {
        return fallback(
            evidence,
            "Not sent to a language model: the evidence does not answer the question.".into(),
            None,
        );
    }
    let Some(config) = model else {
        return fallback(
            evidence,
            "Language model disabled; this explanation is built from the evidence.".into(),
            None,
        );
    };
    match ollama::chat(config, &prompt::messages(&evidence)).await {
        Ok(answer) => {
            let verification = verify::verify(&answer, &evidence);
            if verification.accepted {
                Explanation {
                    evidence,
                    text: answer,
                    source: TextSource::Model,
                    model: Some(config.model.clone()),
                    notes: vec![format!(
                        "Written by {} from the evidence below and checked against it.",
                        config.model
                    )],
                    verification: Some(verification),
                    rejected_text: None,
                }
            } else {
                tracing::info!(problems = ?verification.problems, "model answer rejected");
                Explanation {
                    text: template.clone(),
                    evidence,
                    source: TextSource::Template,
                    model: Some(config.model.clone()),
                    notes: vec![format!(
                        "The answer from {} was rejected ({}); this explanation is built from the evidence.",
                        config.model,
                        verification.problems.join("; ")
                    )],
                    verification: Some(verification),
                    rejected_text: Some(answer),
                }
            }
        }
        Err(err) => fallback(
            evidence,
            format!("No model answer ({err}); this explanation is built from the evidence."),
            Some(config.model.clone()),
        ),
    }
}
