//! Jev (`typesafe-ai/jev`) — a cheap, calibrated classifier served through the
//! Vercel AI Gateway's evaluation-model endpoint (issue #249). It takes a JSON
//! `state` plus typed questions and returns, per question, a choice with class
//! probabilities (and a separate confidence). It never writes prose.
//!
//! First job: a second opinion on each AI finding — is the claimed problem
//! real, and does it block the merge? — measured against the eval corpus
//! (`marrow eval --jev`) before anything reaches the UI.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::net::{backoff_delay, http_client, retryable_response_delay, MAX_ATTEMPTS};
use crate::types::HighlightResult;

pub const JEV_MODEL: &str = "typesafe-ai/jev";
const ENDPOINT: &str = "https://ai-gateway.vercel.sh/v4/ai/evaluation-model";
/// A judgement is a few hundred tokens; this bounds a hung request.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Keep the diff in the state bounded — the claim is about a few lines.
const MAX_DIFF_CHARS: usize = 8_000;
const MAX_BODY_CHARS: usize = 1_500;

/// One multiple-choice question: `criteria` maps each class label to what it
/// means. Serializes to the gateway's question shape.
#[derive(Debug, Clone, Serialize)]
pub struct JevQuestion {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub instructions: String,
    pub criteria: BTreeMap<String, String>,
}

impl JevQuestion {
    pub fn choice(instructions: &str, criteria: &[(&str, &str)]) -> Self {
        JevQuestion {
            kind: "choice",
            instructions: instructions.to_string(),
            criteria: criteria.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }
}

/// Jev's answer to one choice question.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JevChoice {
    pub choice: String,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

fn valid_unit(v: f64) -> bool {
    v.is_finite() && (0.0..=1.0).contains(&v)
}

/// Validate a gateway response against the questions asked: every question
/// answered, classes exactly the criteria labels, probabilities in [0,1]
/// summing to 1, the choice one of the labels, and a confidence present
/// (the gateway carries it in `providerMetadata.typesafe.confidence`).
pub fn parse_response(raw: &Value, questions: &BTreeMap<String, JevQuestion>) -> Result<BTreeMap<String, JevChoice>, String> {
    let answers = raw.get("answers").and_then(Value::as_object).ok_or("Jev response has no answers")?;
    let confidences = raw.pointer("/providerMetadata/typesafe/confidence").and_then(Value::as_object);
    let mut out = BTreeMap::new();
    for (name, q) in questions {
        let a = answers.get(name).ok_or_else(|| format!("Jev didn't answer `{name}`"))?;
        let choice = a.get("choice").and_then(Value::as_str).ok_or_else(|| format!("`{name}`: no choice"))?;
        let probs_raw = a.get("probabilities").and_then(Value::as_object).ok_or_else(|| format!("`{name}`: no probabilities"))?;
        let mut probabilities = BTreeMap::new();
        for (label, p) in probs_raw {
            let p = p.as_f64().filter(|p| valid_unit(*p)).ok_or_else(|| format!("`{name}`: bad probability for `{label}`"))?;
            probabilities.insert(label.clone(), p);
        }
        if probabilities.keys().ne(q.criteria.keys()) {
            return Err(format!("`{name}`: classes don't match the question's labels"));
        }
        let sum: f64 = probabilities.values().sum();
        if (sum - 1.0).abs() > 0.01 {
            return Err(format!("`{name}`: probabilities sum to {sum:.3}"));
        }
        if !q.criteria.contains_key(choice) {
            return Err(format!("`{name}`: choice `{choice}` isn't a label"));
        }
        let confidence = confidences
            .and_then(|c| c.get(name))
            .and_then(Value::as_f64)
            .filter(|c| valid_unit(*c))
            .ok_or_else(|| format!("`{name}`: no confidence in the response"))?;
        out.insert(name.clone(), JevChoice { choice: choice.to_string(), probabilities, confidence });
    }
    Ok(out)
}

/// Ask Jev `questions` about `state`. Retries 429/5xx and transient transport
/// failures (Jev's gateway limit is ~30 requests/minute).
pub async fn evaluate(
    api_key: &str,
    state: &Value,
    questions: &BTreeMap<String, JevQuestion>,
) -> Result<BTreeMap<String, JevChoice>, String> {
    let body = json!({ "state": state, "questions": questions });
    let mut attempt = 0;
    loop {
        attempt += 1;
        let sent = http_client()
            .post(ENDPOINT)
            .timeout(REQUEST_TIMEOUT)
            .bearer_auth(api_key)
            .header("ai-gateway-auth-method", "api-key")
            .header("ai-gateway-protocol-version", "0.0.1")
            .header("ai-evaluation-model-specification-version", "4")
            .header("ai-model-id", JEV_MODEL)
            .json(&body)
            .send()
            .await;
        match sent {
            Ok(resp) if resp.status().is_success() => {
                let raw: Value = resp.json().await.map_err(|e| format!("Jev response wasn't JSON: {e}"))?;
                return parse_response(&raw, questions);
            }
            Ok(resp) => {
                let status = resp.status();
                if attempt < MAX_ATTEMPTS {
                    if let Some(wait) = retryable_response_delay(status, resp.headers(), attempt) {
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                }
                let text = resp.text().await.unwrap_or_default();
                let snippet: String = text.chars().take(200).collect();
                return Err(format!("Jev request failed: HTTP {status} {snippet}"));
            }
            Err(e) if attempt < MAX_ATTEMPTS && crate::net::transient_transport_error(&e) => {
                tokio::time::sleep(backoff_delay(attempt)).await;
            }
            Err(e) => return Err(format!("Jev request failed: {e}")),
        }
    }
}

// ── Finding judge ────────────────────────────────────────────────────────

pub const Q_DEFECT: &str = "defect";
pub const Q_URGENCY: &str = "urgency";

/// The two questions asked of every finding.
pub fn finding_questions() -> BTreeMap<String, JevQuestion> {
    let mut q = BTreeMap::new();
    q.insert(
        Q_DEFECT.to_string(),
        JevQuestion::choice(
            "An AI code reviewer made the claim in `finding` about the changed code in `diff`. \
             Judge the claim against the diff and the PR's stated intent: is the problem it \
             describes actually present?",
            &[
                ("real", "The changed code really has the problem described: wrong behavior, a bug, a missing case, or a genuine gap."),
                ("not_real", "The claim misreads the code, describes behavior the PR intends, or is a matter of style or preference."),
            ],
        ),
    );
    q.insert(
        Q_URGENCY.to_string(),
        JevQuestion::choice(
            "How should the reviewer treat this finding before the PR merges?",
            &[
                ("fix_before_merge", "A real defect that would ship broken or wrong behavior; the author should fix it before merging."),
                ("worth_a_look", "A plausible concern, minor issue, or missing test; worth the reviewer's attention but not blocking."),
                ("noise", "Not worth the reviewer's time: incorrect, intended by the PR, or trivial."),
            ],
        ),
    );
    q
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max).collect();
    t.push_str("\n… (truncated)");
    t
}

/// What Jev sees for one finding: the PR's title and (trimmed) description,
/// the finding itself, and the diff of the file it's in.
pub fn finding_state(pr_title: &str, pr_body: &str, h: &HighlightResult, file_diff: &str) -> Value {
    json!({
        "pr": { "title": pr_title, "description": truncate(pr_body.trim(), MAX_BODY_CHARS) },
        "finding": {
            "file": h.path,
            "lines": format!("{}-{}", h.start_line, h.end_line),
            "category": h.category,
            "severity": h.severity,
            "claim": h.comment,
            "scenario": h.scenario,
            "suggested_fix": h.fix,
        },
        "diff": truncate(file_diff, MAX_DIFF_CHARS),
    })
}

/// Jev's second opinion on one finding.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FindingJudgement {
    /// P(the claimed problem is real).
    pub p_real: f64,
    pub defect_confidence: f64,
    /// The most likely urgency class and every class's probability.
    pub urgency: String,
    pub p_fix: f64,
    pub p_look: f64,
    pub p_noise: f64,
    pub urgency_confidence: f64,
}

pub fn judgement_from(answers: &BTreeMap<String, JevChoice>) -> Result<FindingJudgement, String> {
    let d = answers.get(Q_DEFECT).ok_or("no defect answer")?;
    let u = answers.get(Q_URGENCY).ok_or("no urgency answer")?;
    let p = |c: &JevChoice, k: &str| c.probabilities.get(k).copied().unwrap_or(0.0);
    Ok(FindingJudgement {
        p_real: p(d, "real"),
        defect_confidence: d.confidence,
        urgency: u.choice.clone(),
        p_fix: p(u, "fix_before_merge"),
        p_look: p(u, "worth_a_look"),
        p_noise: p(u, "noise"),
        urgency_confidence: u.confidence,
    })
}

pub async fn judge_finding(
    api_key: &str,
    pr_title: &str,
    pr_body: &str,
    h: &HighlightResult,
    file_diff: &str,
) -> Result<FindingJudgement, String> {
    let questions = finding_questions();
    let answers = evaluate(api_key, &finding_state(pr_title, pr_body, h, file_diff), &questions).await?;
    judgement_from(&answers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(defect: (&str, f64), urgency: (&str, [f64; 3])) -> Value {
        json!({
            "answers": {
                "defect": { "type": "choice", "choice": defect.0,
                    "probabilities": { "real": defect.1, "not_real": 1.0 - defect.1 } },
                "urgency": { "type": "choice", "choice": urgency.0,
                    "probabilities": { "fix_before_merge": urgency.1[0], "worth_a_look": urgency.1[1], "noise": urgency.1[2] } }
            },
            "providerMetadata": { "typesafe": { "confidence": { "defect": 0.8, "urgency": 0.6 } } }
        })
    }

    #[test]
    fn a_valid_response_becomes_a_judgement() {
        let raw = response(("real", 0.9), ("fix_before_merge", [0.7, 0.2, 0.1]));
        let j = judgement_from(&parse_response(&raw, &finding_questions()).unwrap()).unwrap();
        assert_eq!(j.urgency, "fix_before_merge");
        assert!((j.p_real - 0.9).abs() < 1e-9);
        assert!((j.p_noise - 0.1).abs() < 1e-9);
        assert!((j.defect_confidence - 0.8).abs() < 1e-9);
    }

    #[test]
    fn responses_that_dont_match_the_questions_are_rejected() {
        let q = finding_questions();
        // Probabilities that don't sum to 1.
        let mut raw = response(("real", 0.9), ("noise", [0.5, 0.5, 0.5]));
        assert!(parse_response(&raw, &q).unwrap_err().contains("sum"));
        // A choice that isn't one of the labels.
        raw = response(("maybe", 0.5), ("noise", [0.1, 0.1, 0.8]));
        assert!(parse_response(&raw, &q).unwrap_err().contains("isn't a label"));
        // A missing class.
        raw = response(("real", 0.9), ("noise", [0.1, 0.1, 0.8]));
        raw["answers"]["urgency"]["probabilities"].as_object_mut().unwrap().remove("worth_a_look");
        assert!(parse_response(&raw, &q).is_err());
        // No confidence metadata.
        raw = response(("real", 0.9), ("noise", [0.1, 0.1, 0.8]));
        raw["providerMetadata"] = json!({});
        assert!(parse_response(&raw, &q).unwrap_err().contains("confidence"));
        // An unanswered question.
        raw = response(("real", 0.9), ("noise", [0.1, 0.1, 0.8]));
        raw["answers"].as_object_mut().unwrap().remove("defect");
        assert!(parse_response(&raw, &q).unwrap_err().contains("didn't answer"));
    }

    #[test]
    fn the_state_carries_the_finding_and_a_bounded_diff() {
        let h = HighlightResult {
            path: "src/a.rs".into(),
            start_line: 10,
            end_line: 12,
            severity: "high".into(),
            comment: "Off-by-one drops the last item.".into(),
            category: "bug".into(),
            scenario: "A 3-item list shows 2.".into(),
            fix: "Use ..= instead of ..".into(),
        };
        let s = finding_state("Paginate", "Adds paging.", &h, &"+x\n".repeat(10_000));
        assert_eq!(s["finding"]["lines"], "10-12");
        assert_eq!(s["finding"]["claim"], "Off-by-one drops the last item.");
        let diff = s["diff"].as_str().unwrap();
        assert!(diff.chars().count() < MAX_DIFF_CHARS + 20 && diff.ends_with("(truncated)"));
    }

    #[test]
    fn questions_serialize_to_the_gateway_shape() {
        let v = serde_json::to_value(finding_questions()).unwrap();
        assert_eq!(v["urgency"]["type"], "choice");
        assert!(v["defect"]["criteria"]["real"].is_string());
    }
}
