//! Evidence on the fixtures, and the whole explanation flow against a
//! stand-in for Ollama that records requests and returns scripted answers.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use codeatlas_analyzer::graph::impact::ImpactOptions;
use codeatlas_analyzer::graph::CodeGraph;
use codeatlas_analyzer::ingest::IngestOptions;
use codeatlas_analyzer::model::SymbolId;
use codeatlas_analyzer::{analyze_source, RepoSource};
use codeatlas_explain::evidence::{impact, why};
use codeatlas_explain::{explain, EvidenceStatus, ModelConfig, TextSource};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn graph(fixture: &str) -> CodeGraph {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(fixture);
    let analysis = analyze_source(&RepoSource::Local(path), &IngestOptions::default()).unwrap();
    CodeGraph::from_analysis(&analysis)
}

fn id(s: &str) -> SymbolId {
    SymbolId::from_stored(s.to_string())
}

const VALIDATE: &str = "fn:change_impact::payments::validate_amount";
const TEST_CHECKOUT: &str = "fn:payment_tests::test_checkout";

/// A stand-in model server: answers every request with `status` and
/// `body`, and records the request bodies.
async fn model_server(status: u16, body: String) -> (ModelConfig, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 4096];
            // Read the headers, then the announced body.
            let request = loop {
                let n = socket.read(&mut chunk).await.unwrap();
                buffer.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buffer).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if buffer.len() >= end + 4 + length {
                        break text[end + 4..end + 4 + length].to_string();
                    }
                }
                if n == 0 {
                    break String::new();
                }
            };
            seen.lock().unwrap().push(request);
            let response = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    (
        ModelConfig {
            url,
            model: "test-model".into(),
            timeout: Duration::from_secs(5),
        },
        requests,
    )
}

fn answer(text: &str) -> String {
    serde_json::json!({ "message": { "role": "assistant", "content": text }, "done": true })
        .to_string()
}

#[test]
fn why_follows_the_chain_with_files_and_lines() {
    let g = graph("change-impact");
    let e = why(
        &g,
        &id(VALIDATE),
        &id(TEST_CHECKOUT),
        ImpactOptions::default(),
    )
    .unwrap();
    assert_eq!(e.status, EvidenceStatus::Sufficient);
    let facts: Vec<&str> = e.facts.iter().map(|f| f.text.as_str()).collect();
    assert_eq!(
        facts,
        [
            "`test_checkout` calls `checkout` (tests/payment_tests.rs:16).",
            "`checkout` calls `PaymentService::authorize` (src/checkout.rs:5).",
            "`PaymentService::authorize` calls `validate_amount` (src/payments.rs:13).",
        ]
    );
    assert_eq!(e.chains, [vec!["E1", "E2", "E3"]]);
    let text = codeatlas_explain::template::render(&e);
    assert!(text.contains("[E1]") && text.contains("[E3]"), "{text}");
}

#[test]
fn why_says_when_there_is_no_evidence_or_it_runs_the_other_way() {
    let g = graph("change-impact");
    let none = why(
        &g,
        &id("fn:change_impact::reports::daily_total"),
        &id(TEST_CHECKOUT),
        ImpactOptions::default(),
    )
    .unwrap();
    assert_eq!(none.status, EvidenceStatus::Insufficient);
    assert!(none.facts.is_empty());
    assert!(none.reason.unwrap().contains("no chain"));

    // `validate_amount` does not depend on `checkout`; the reverse holds.
    let reverse = why(
        &g,
        &id("fn:change_impact::checkout::checkout"),
        &id(VALIDATE),
        ImpactOptions::default(),
    )
    .unwrap();
    assert_eq!(reverse.status, EvidenceStatus::Insufficient);
    assert!(reverse.reason.unwrap().contains("runs the other way"));
    assert_eq!(reverse.chains.len(), 1);
}

#[test]
fn why_marks_chains_through_ambiguous_calls_as_possible() {
    let g = graph("test-impact");
    let e = why(
        &g,
        &id("method:ledger::store::<MemoryStore as Store>::get"),
        &id("fn:ledger_tests::generic_lookup_defaults_to_zero"),
        ImpactOptions::default(),
    )
    .unwrap();
    assert_eq!(e.status, EvidenceStatus::Possible);
    assert!(e
        .facts
        .iter()
        .any(|f| f.text.contains("could not pin down")));
}

#[test]
fn impact_evidence_has_counts_and_chains() {
    let g = graph("change-impact");
    let e = impact(&g, &id(VALIDATE), ImpactOptions::default()).unwrap();
    assert_eq!(e.status, EvidenceStatus::Sufficient);
    assert!(e.facts[0].text.starts_with("4 symbols can be affected"));
    assert!(e.facts[1].text.starts_with("2 tests reach"));
    assert_eq!(e.chains.len(), 4);
}

#[tokio::test]
async fn faithful_answers_are_accepted_and_only_evidence_is_sent() {
    let g = graph("change-impact");
    let e = why(
        &g,
        &id(VALIDATE),
        &id(TEST_CHECKOUT),
        ImpactOptions::default(),
    )
    .unwrap();
    let (config, requests) = model_server(
        200,
        answer(
            "A change to `validate_amount` can change `test_checkout`: the test calls `checkout` [E1], \
             which calls `PaymentService::authorize` [E2], which calls `validate_amount` [E3].",
        ),
    )
    .await;
    let x = explain(e, Some(&config)).await;
    assert_eq!(x.source, TextSource::Model, "{:?}", x.notes);
    assert!(x.verification.as_ref().unwrap().accepted);
    assert_eq!(x.verification.unwrap().cited, ["E1", "E2", "E3"]);

    let sent: serde_json::Value = serde_json::from_str(&requests.lock().unwrap()[0]).unwrap();
    assert_eq!(sent["model"], "test-model");
    assert_eq!(sent["options"]["temperature"], 0);
    let user = sent["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains("[E2] `checkout` calls `PaymentService::authorize` (src/checkout.rs:5)."));
    // Nothing from the repository beyond the facts: no source code.
    assert!(!user.contains("fn validate_amount"));
}

#[tokio::test]
async fn invented_relationships_and_bad_citations_are_rejected() {
    let g = graph("change-impact");
    for (bad, problem) in [
        (
            "`test_checkout` calls `refund_order` [E1], which calls `validate_amount` [E3].",
            "`refund_order`",
        ),
        ("`checkout` calls `validate_amount` directly [E7].", "E7"),
        ("The test depends on the validation logic.", "cites none"),
    ] {
        let e = why(
            &g,
            &id(VALIDATE),
            &id(TEST_CHECKOUT),
            ImpactOptions::default(),
        )
        .unwrap();
        let (config, _) = model_server(200, answer(bad)).await;
        let x = explain(e, Some(&config)).await;
        assert_eq!(x.source, TextSource::Template, "accepted: {bad}");
        assert_eq!(x.rejected_text.as_deref(), Some(bad));
        let problems = x.verification.unwrap().problems.join(" ");
        assert!(problems.contains(problem), "{bad}: {problems}");
        assert!(x.text.contains("[E1]"), "falls back to the template");
    }
}

#[tokio::test]
async fn missing_models_and_servers_fall_back_to_the_template() {
    let g = graph("change-impact");
    let e = why(
        &g,
        &id(VALIDATE),
        &id(TEST_CHECKOUT),
        ImpactOptions::default(),
    )
    .unwrap();
    let (config, _) = model_server(
        404,
        r#"{"error":"model 'test-model' not found"}"#.to_string(),
    )
    .await;
    let x = explain(e.clone(), Some(&config)).await;
    assert_eq!(x.source, TextSource::Template);
    assert!(
        x.notes[0].contains("ollama pull test-model"),
        "{:?}",
        x.notes
    );

    let down = ModelConfig {
        url: "http://127.0.0.1:9".into(),
        model: "test-model".into(),
        timeout: Duration::from_secs(2),
    };
    let x = explain(e.clone(), Some(&down)).await;
    assert_eq!(x.source, TextSource::Template);
    assert!(x.notes[0].contains("No model answer"), "{:?}", x.notes);

    let x = explain(e, None).await;
    assert!(x.notes[0].contains("disabled"));
}

#[tokio::test]
async fn insufficient_evidence_is_never_sent_to_the_model() {
    let g = graph("change-impact");
    let e = why(
        &g,
        &id("fn:change_impact::reports::daily_total"),
        &id(TEST_CHECKOUT),
        ImpactOptions::default(),
    )
    .unwrap();
    let (config, requests) = model_server(200, answer("Anything [E1].")).await;
    let x = explain(e, Some(&config)).await;
    assert_eq!(x.source, TextSource::Template);
    assert!(x.text.starts_with("CodeAtlas has no evidence for this"));
    assert!(requests.lock().unwrap().is_empty());
}
