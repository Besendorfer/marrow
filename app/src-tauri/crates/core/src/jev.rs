//! Jev — TypeSafe's cheap, calibrated classifier, called through TypeSafe's
//! own API (`POST https://api.typesafe.ai/v1/systemone`, issue #249). It takes
//! a JSON `state` plus typed questions and returns, per question, a choice
//! with class probabilities and a confidence. It never writes prose.
//!
//! First job: a second opinion on each AI finding — is the claimed problem
//! real, and does it block the merge? — measured against the eval corpus
//! (`marrow eval --jev`) before anything reaches the UI.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::net::{backoff_delay, http_client, retryable_response_delay, MAX_ATTEMPTS};
use crate::types::{FindingRef, FindingRelation, HighlightResult};
use std::collections::HashMap;

/// TypeSafe's flagship Jev model.
pub const JEV_MODEL: &str = "jev-latest";
const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
/// A judgement is a few hundred tokens; this bounds a hung request.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Keep the diff in the state bounded — the claim is about a few lines.
const MAX_DIFF_CHARS: usize = 8_000;
const MAX_BODY_CHARS: usize = 1_500;

/// One multiple-choice question: `criteria` maps each class label to what it
/// means. Serializes to TypeSafe's `choice` question shape.
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

    /// A yes/no question (TypeSafe's `noul`); its answer is P(true).
    pub fn noul(instructions: &str, if_true: &str, if_false: &str) -> Self {
        JevQuestion {
            kind: "noul",
            instructions: instructions.to_string(),
            criteria: [("true".to_string(), if_true.to_string()), ("false".to_string(), if_false.to_string())].into(),
        }
    }
}

/// Jev's answer to one question. A yes/no (`noul`) answer is expressed the
/// same way — choice "true"/"false", probabilities for both — and carries no
/// confidence (TypeSafe returns only the probability).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JevChoice {
    pub choice: String,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: Option<f64>,
}

fn valid_unit(v: f64) -> bool {
    v.is_finite() && (0.0..=1.0).contains(&v)
}

/// Validate a TypeSafe response against the questions asked: every question
/// answered, classes exactly the criteria labels, probabilities in [0,1]
/// summing to 1, the choice one of the labels, and a confidence in [0,1].
pub fn parse_response(raw: &Value, questions: &BTreeMap<String, JevQuestion>) -> Result<BTreeMap<String, JevChoice>, String> {
    let answers = raw.get("answers").and_then(Value::as_object).ok_or("Jev response has no answers")?;
    let mut out = BTreeMap::new();
    for (name, q) in questions {
        let a = answers.get(name).ok_or_else(|| format!("Jev didn't answer `{name}`"))?;
        if q.kind == "noul" {
            let p = a.get("noul").and_then(Value::as_f64).filter(|p| valid_unit(*p)).ok_or_else(|| format!("`{name}`: no yes/no probability"))?;
            let choice = if p >= 0.5 { "true" } else { "false" };
            let probabilities = [("true".to_string(), p), ("false".to_string(), 1.0 - p)].into();
            out.insert(name.clone(), JevChoice { choice: choice.to_string(), probabilities, confidence: None });
            continue;
        }
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
        // TypeSafe rounds to two decimals, so three classes can sum to 0.99.
        if (sum - 1.0).abs() > 0.02 {
            return Err(format!("`{name}`: probabilities sum to {sum:.3}"));
        }
        if !q.criteria.contains_key(choice) {
            return Err(format!("`{name}`: choice `{choice}` isn't a label"));
        }
        let confidence = a
            .get("confidence")
            .and_then(Value::as_f64)
            .filter(|c| valid_unit(*c))
            .ok_or_else(|| format!("`{name}`: no confidence in the response"))?;
        out.insert(name.clone(), JevChoice { choice: choice.to_string(), probabilities, confidence: Some(confidence) });
    }
    Ok(out)
}

/// Ask Jev `questions` about `state`. Retries 429 (rate limited), 5xx
/// including 529 (overloaded), and transient transport failures, with
/// backoff — what TypeSafe's docs ask of clients.
pub async fn evaluate(
    api_key: &str,
    state: &Value,
    questions: &BTreeMap<String, JevQuestion>,
) -> Result<BTreeMap<String, JevChoice>, String> {
    let body = json!({ "model": JEV_MODEL, "state": state, "questions": questions });
    let mut attempt = 0;
    loop {
        attempt += 1;
        let sent = http_client()
            .post(ENDPOINT)
            .timeout(REQUEST_TIMEOUT)
            .bearer_auth(api_key)
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

pub const Q_INTENDED: &str = "intended";

/// The tuned question set (issue #249, round 2): `defect` asks Jev to check
/// the claim against the cited lines and to treat a restatement of the PR's
/// intended change as not real, and a separate yes/no asks whether the
/// finding just flags the PR's stated intent.
pub fn finding_questions_v2() -> BTreeMap<String, JevQuestion> {
    let mut q = finding_questions();
    q.insert(
        Q_DEFECT.to_string(),
        JevQuestion::choice(
            "An AI code reviewer made the claim in `finding` about lines `finding.lines` of \
             `finding.file`. Check it against `diff` (a unified diff: `+` lines are new, `-` \
             lines are removed), the PR description, and any `evidence` (code outside the diff \
             that the reviewer read). Answer `real` only if the diff or the evidence shows the \
             problem. A claim that restates a change the PR description says it makes on \
             purpose, without naming a concrete breakage beyond it, is `not_real`.",
            &[
                ("real", "The diff or the evidence shows the claimed problem: wrong behavior, a bug, a missing case, or an unintended change, at the cited lines or in code they affect."),
                ("not_real", "Neither shows it: the claim misreads the code, cites lines where it doesn't happen, invents something the code doesn't do, restates the PR's intended change, or is a style preference."),
            ],
        ),
    );
    q.insert(
        Q_INTENDED.to_string(),
        JevQuestion::noul(
            "Is the change this finding flags exactly what the PR description says the PR sets out to do?",
            "Yes: the finding flags the PR's stated, intended change itself.",
            "No: it flags something beyond or different from the stated intent.",
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
    finding_state_with_evidence(pr_title, pr_body, h, file_diff, &[])
}

/// Most of a file's worth of evidence, and a few files at most.
const MAX_EVIDENCE_CHARS: usize = 4_000;
const MAX_EVIDENCE_FILES: usize = 4;

/// `finding_state` plus `evidence`: files outside the diff that the review
/// read (path, content), for claims only knowable from beyond the diff.
pub fn finding_state_with_evidence(
    pr_title: &str,
    pr_body: &str,
    h: &HighlightResult,
    file_diff: &str,
    evidence: &[(String, String)],
) -> Value {
    let mut state = json!({
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
    });
    if !evidence.is_empty() {
        state["evidence"] = evidence
            .iter()
            .take(MAX_EVIDENCE_FILES)
            .map(|(path, content)| json!({ "file": path, "content": truncate(content, MAX_EVIDENCE_CHARS) }))
            .collect();
    }
    state
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
        defect_confidence: d.confidence.unwrap_or(0.0),
        urgency: u.choice.clone(),
        p_fix: p(u, "fix_before_merge"),
        p_look: p(u, "worth_a_look"),
        p_noise: p(u, "noise"),
        urgency_confidence: u.confidence.unwrap_or(0.0),
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

// ── File relevance (issue #249, second use) ─────────────────────────────

pub const Q_RELEVANT: &str = "relevant";
pub const Q_RISK: &str = "risk";
pub const Q_CATEGORY: &str = "category";

/// The rules half of the LLM's CLASSIFICATION_PROMPT (everything before its
/// output-format section), so Jev and the LLM classify by the same text.
pub fn classification_rules() -> &'static str {
    let p = crate::prompts::CLASSIFICATION_PROMPT;
    p.split("Respond with ONLY").next().unwrap_or(p).trim()
}

/// Three questions per file, one call: everything the app reads from a
/// classification (relevance, risk level, category).
pub fn file_questions() -> BTreeMap<String, JevQuestion> {
    let mut q = BTreeMap::new();
    q.insert(
        Q_RELEVANT.to_string(),
        JevQuestion::choice(
            &format!(
                "Classify the changed file in `file` (its diff is `diff`; `pr` describes the pull request and \
                 `other_files` lists the rest of its changed files) by these rules:\n\n{}",
                classification_rules()
            ),
            &[
                ("relevant", "RELEVANT under the rules: business logic, infrastructure, API, schema, auth, runtime config, or a shared library with logic."),
                ("not_relevant", "NOT_RELEVANT under the rules: tests, docs, presentational UI, tooling/build config, lockfiles, pure re-export barrels, assets, or generated files."),
            ],
        ),
    );
    q.insert(
        Q_RISK.to_string(),
        JevQuestion::choice(
            "How much could this file's change hurt if it's wrong? (Tests, docs, and other not-relevant files are low.)",
            &[
                ("critical", "Security-sensitive: auth, payment/billing, data deletion, database migrations, IAM/permissions."),
                ("high", "Core business logic, API contract changes, infrastructure, or shared libraries many callers use."),
                ("medium", "Standard feature code, service implementations, non-critical handlers."),
                ("low", "Minor refactors, logging, comments, config tweaks, test helpers, or not-relevant files."),
            ],
        ),
    );
    q.insert(
        Q_CATEGORY.to_string(),
        JevQuestion::choice(
            "What kind of code is this file?",
            &[
                ("business_logic", "Business logic: services, handlers, models, validation, domain rules."),
                ("infrastructure", "Infrastructure: IaC, CI/CD, deployment, runtime configuration."),
                ("domain_types", "Domain types: type definitions, schemas, DTOs."),
                ("other", "Anything else."),
            ],
        ),
    );
    q
}

/// What Jev sees for one file.
pub fn file_state(pr_title: &str, pr_body: &str, path: &str, all_paths: &[String], diff: &str) -> Value {
    let others: Vec<&String> = all_paths.iter().filter(|p| p.as_str() != path).take(50).collect();
    json!({
        "pr": { "title": pr_title, "description": truncate(pr_body.trim(), MAX_BODY_CHARS) },
        "file": path,
        "other_files": others,
        "diff": truncate(diff, MAX_DIFF_CHARS),
    })
}

/// Jev's classification of one file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileJudgement {
    pub p_relevant: f64,
    pub relevance_confidence: f64,
    pub risk: String,
    pub category: String,
}

impl FileJudgement {
    pub fn relevant(&self) -> bool {
        self.p_relevant >= 0.5
    }

    /// In the LLM pass's shape, so it can go through validate_classifications
    /// and everything downstream unchanged.
    pub fn to_classification(&self, path: &str) -> crate::types::FileClassification {
        let relevant = self.relevant();
        crate::types::FileClassification {
            path: path.to_string(),
            classification: if relevant { "RELEVANT" } else { "NOT_RELEVANT" }.to_string(),
            category: if !relevant {
                "N/A".to_string()
            } else {
                match self.category.as_str() {
                    "business_logic" => "Business Logic",
                    "infrastructure" => "Infrastructure",
                    "domain_types" => "Domain Types",
                    _ => "Other",
                }
                .to_string()
            },
            risk_level: if relevant { self.risk.clone() } else { "low".to_string() },
            reason: format!("Jev: P(relevant) {:.2}", self.p_relevant),
        }
    }
}

pub fn file_judgement_from(answers: &BTreeMap<String, JevChoice>) -> Result<FileJudgement, String> {
    let r = answers.get(Q_RELEVANT).ok_or("no relevance answer")?;
    Ok(FileJudgement {
        p_relevant: r.probabilities.get("relevant").copied().unwrap_or(0.0),
        relevance_confidence: r.confidence.unwrap_or(0.0),
        risk: answers.get(Q_RISK).map(|c| c.choice.clone()).unwrap_or_else(|| "low".to_string()),
        category: answers.get(Q_CATEGORY).map(|c| c.choice.clone()).unwrap_or_else(|| "other".to_string()),
    })
}

pub async fn classify_file(
    api_key: &str,
    pr_title: &str,
    pr_body: &str,
    path: &str,
    all_paths: &[String],
    diff: &str,
) -> Result<FileJudgement, String> {
    let questions = file_questions();
    let answers = evaluate(api_key, &file_state(pr_title, pr_body, path, all_paths, diff), &questions).await?;
    file_judgement_from(&answers)
}

// ── Duplicate findings (issue #249, third use) ──────────────────────────

pub const Q_SAME: &str = "same";

/// Is finding `b` the same problem as finding `a`, the same root cause with
/// a different action, or a separate problem?
pub fn pair_questions() -> BTreeMap<String, JevQuestion> {
    let mut q = BTreeMap::new();
    q.insert(
        Q_SAME.to_string(),
        JevQuestion::choice(
            "An AI code reviewer reported `finding_a` and `finding_b` on this pull request (`diff` holds \
             the diffs of the files they're in). Decide how they relate, judging by the underlying \
             problem each describes, not by wording, category, or exact lines.",
            &[
                ("same_issue", "Both describe one problem, fixed by one change: the same defect or behavior change, said twice."),
                ("related", "One root cause, different actions: e.g. a bug and the missing test for that bug, or a defect and its consequence elsewhere that needs its own change."),
                ("different", "Separate problems that each need their own fix, even if they're near each other."),
            ],
        ),
    );
    q
}

fn finding_json(h: &HighlightResult) -> Value {
    json!({
        "file": h.path,
        "lines": format!("{}-{}", h.start_line, h.end_line),
        "category": h.category,
        "claim": h.comment,
    })
}

/// What Jev sees for a pair: the PR, both findings, and the diff of each
/// file involved (once if they share a file).
pub fn pair_state(pr_title: &str, pr_body: &str, a: &HighlightResult, b: &HighlightResult, diffs: &[(String, String)]) -> Value {
    let per_file = MAX_DIFF_CHARS / diffs.len().max(1);
    json!({
        "pr": { "title": pr_title, "description": truncate(pr_body.trim(), MAX_BODY_CHARS) },
        "finding_a": finding_json(a),
        "finding_b": finding_json(b),
        "diff": diffs.iter().map(|(p, d)| json!({ "file": p, "diff": truncate(d, per_file) })).collect::<Vec<_>>(),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PairJudgement {
    pub relation: String,
    pub p_same: f64,
    pub p_related: f64,
    pub p_different: f64,
    pub confidence: f64,
}

pub fn pair_judgement_from(answers: &BTreeMap<String, JevChoice>) -> Result<PairJudgement, String> {
    let c = answers.get(Q_SAME).ok_or("no pair answer")?;
    let p = |k: &str| c.probabilities.get(k).copied().unwrap_or(0.0);
    Ok(PairJudgement {
        relation: c.choice.clone(),
        p_same: p("same_issue"),
        p_related: p("related"),
        p_different: p("different"),
        confidence: c.confidence.unwrap_or(0.0),
    })
}

pub async fn judge_pair(
    api_key: &str,
    pr_title: &str,
    pr_body: &str,
    a: &HighlightResult,
    b: &HighlightResult,
    diffs: &[(String, String)],
) -> Result<PairJudgement, String> {
    let answers = evaluate(api_key, &pair_state(pr_title, pr_body, a, b, diffs), &pair_questions()).await?;
    pair_judgement_from(&answers)
}

/// Findings closer than this (in lines, same file) are candidate duplicates.
pub const PAIR_WINDOW: u64 = 15;
/// At most this many pairs are judged per PR, nearest first.
pub const MAX_PAIRS: usize = 12;
const PAIR_CONCURRENCY: usize = 4;

/// Mirrors the frontend's highlightRank: an info note with no actionable
/// category stays inline in the diff and never becomes a list finding.
fn is_list_finding(h: &HighlightResult) -> bool {
    h.severity != "info" || (!h.category.is_empty() && h.category != "observation")
}

/// Pairs of list findings in the same file within PAIR_WINDOW lines of each
/// other, nearest first, capped at MAX_PAIRS.
pub fn candidate_pairs(hs: &[HighlightResult]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for i in 0..hs.len() {
        for j in (i + 1)..hs.len() {
            let (a, b) = (&hs[i], &hs[j]);
            if a.path != b.path || !is_list_finding(a) || !is_list_finding(b) {
                continue;
            }
            let gap = a.start_line.max(b.start_line).saturating_sub(a.end_line.min(b.end_line));
            if gap <= PAIR_WINDOW {
                pairs.push((gap, i, j));
            }
        }
    }
    pairs.sort();
    pairs.into_iter().take(MAX_PAIRS).map(|(_, i, j)| (i, j)).collect()
}

fn finding_ref(h: &HighlightResult) -> FindingRef {
    FindingRef { path: h.path.clone(), start_line: h.start_line, end_line: h.end_line, comment: h.comment.clone() }
}

/// A merge gives two findings one shared verdict, so it needs a confident
/// call; a weaker "same" is kept as "related" (grouped, both still acted on
/// separately). On the eval's real output a 0.62 "same" joined two distinct
/// bugs on one line; true duplicates in the probe set scored 0.69–1.00.
pub const SAME_MIN: f64 = 0.8;

/// Keep Jev's "same" and "related" calls as manifest relations.
pub fn relation_from(a: &HighlightResult, b: &HighlightResult, j: &PairJudgement) -> Option<FindingRelation> {
    let relation = match j.relation.as_str() {
        "same_issue" if j.p_same >= SAME_MIN => "same",
        "same_issue" | "related" => "related",
        _ => return None,
    };
    Some(FindingRelation { a: finding_ref(a), b: finding_ref(b), relation: relation.to_string(), p_same: j.p_same, p_related: j.p_related })
}

/// Judge nearby finding pairs. Best effort: no key, or any failed call, just
/// means fewer relations — the review never waits on or fails because of Jev.
pub async fn relate_findings(
    api_key: Option<&str>,
    pr_title: &str,
    pr_body: &str,
    hs: &[HighlightResult],
    diffs: &HashMap<String, String>,
) -> Vec<FindingRelation> {
    use futures::stream::{self, StreamExt};
    let Some(key) = api_key else { return Vec::new() };
    let pairs = candidate_pairs(hs);
    stream::iter(pairs)
        .map(|(i, j)| async move {
            let (a, b) = (&hs[i], &hs[j]);
            let diff = vec![(a.path.clone(), diffs.get(&a.path).cloned().unwrap_or_default())];
            judge_pair(key, pr_title, pr_body, a, b, &diff).await.ok().and_then(|jd| relation_from(a, b, &jd))
        })
        .buffered(PAIR_CONCURRENCY)
        .filter_map(|r| async move { r })
        .collect()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    // TypeSafe's documented response shape: confidence rides on each answer.
    fn response(defect: (&str, f64), urgency: (&str, [f64; 3])) -> Value {
        json!({
            "model": "jev-latest",
            "answers": {
                "defect": { "type": "choice", "choice": defect.0, "confidence": 0.8,
                    "probabilities": { "real": defect.1, "not_real": 1.0 - defect.1 } },
                "urgency": { "type": "choice", "choice": urgency.0, "confidence": 0.6,
                    "probabilities": { "fix_before_merge": urgency.1[0], "worth_a_look": urgency.1[1], "noise": urgency.1[2] } }
            },
            "usage": { "input_tokens": 120, "output_tokens": 4 }
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
        // No confidence on an answer.
        raw = response(("real", 0.9), ("noise", [0.1, 0.1, 0.8]));
        raw["answers"]["defect"].as_object_mut().unwrap().remove("confidence");
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
        assert!(s.get("evidence").is_none());
        let ev = vec![("svc/jobs/nightly.py".to_string(), "except KeyError:".to_string())];
        let s = finding_state_with_evidence("t", "b", &h, "d", &ev);
        assert_eq!(s["evidence"][0]["file"], "svc/jobs/nightly.py");
    }

    #[test]
    fn a_yes_no_answer_parses_to_true_false_probabilities() {
        let q = finding_questions_v2();
        let mut raw = response(("real", 0.9), ("noise", [0.1, 0.1, 0.8]));
        raw["answers"]["intended"] = json!({ "type": "noul", "noul": 0.8 });
        let a = parse_response(&raw, &q).unwrap();
        assert_eq!(a["intended"].choice, "true");
        assert!((a["intended"].probabilities["false"] - 0.2).abs() < 1e-9);
        assert_eq!(a["intended"].confidence, None);
        raw["answers"]["intended"] = json!({ "type": "noul", "noul": 1.5 });
        assert!(parse_response(&raw, &q).unwrap_err().contains("yes/no"));
        let v = serde_json::to_value(&q).unwrap();
        assert_eq!(v["intended"]["type"], "noul");
        assert!(v["intended"]["criteria"]["true"].is_string());
    }

    #[test]
    fn file_questions_carry_the_llm_rules_without_its_output_format() {
        let rules = classification_rules();
        assert!(rules.contains("Test files are ALWAYS NOT_RELEVANT"));
        assert!(!rules.contains("Respond with ONLY"));
        let q = file_questions();
        assert!(q[Q_RELEVANT].instructions.contains(rules));
        assert_eq!(q[Q_RISK].criteria.len(), 4);
    }

    #[test]
    fn a_file_judgement_maps_onto_the_llm_classification_shape() {
        let mut j = FileJudgement { p_relevant: 0.8, relevance_confidence: 0.7, risk: "high".into(), category: "business_logic".into() };
        let c = j.to_classification("src/a.rs");
        assert_eq!((c.classification.as_str(), c.category.as_str(), c.risk_level.as_str()), ("RELEVANT", "Business Logic", "high"));
        j.p_relevant = 0.2;
        let c = j.to_classification("src/a.rs");
        // Not-relevant files are always N/A and low, as the LLM pass is told.
        assert_eq!((c.classification.as_str(), c.category.as_str(), c.risk_level.as_str()), ("NOT_RELEVANT", "N/A", "low"));
        let s = file_state("t", "b", "a.rs", &["a.rs".into(), "b.rs".into()], "d");
        assert_eq!(s["other_files"], json!(["b.rs"]));
    }

    #[test]
    fn a_pair_state_shows_both_findings_and_each_file_once() {
        let h = |path: &str, c: &str| HighlightResult { path: path.into(), start_line: 1, end_line: 2, comment: c.into(), category: "bug".into(), ..Default::default() };
        let s = pair_state("t", "b", &h("a.rs", "x"), &h("a.rs", "y"), &[("a.rs".into(), "d".into())]);
        assert_eq!(s["finding_a"]["claim"], "x");
        assert_eq!(s["finding_b"]["lines"], "1-2");
        assert_eq!(s["diff"].as_array().unwrap().len(), 1);
        assert_eq!(pair_questions()[Q_SAME].criteria.len(), 3);
    }

    #[test]
    fn candidate_pairs_are_nearby_list_findings_in_one_file_nearest_first() {
        let h = |path: &str, s: u64, e: u64, sev: &str, cat: &str| HighlightResult {
            path: path.into(), start_line: s, end_line: e, severity: sev.into(), category: cat.into(), ..Default::default()
        };
        let hs = vec![
            h("a.rs", 10, 12, "warning", "bug"),
            h("a.rs", 40, 41, "warning", "bug"),      // 28 lines from #0: too far
            h("a.rs", 20, 22, "warning", "test_gap"), // 8 from #0, 18 from #1
            h("b.rs", 10, 12, "warning", "bug"),      // other file
            h("a.rs", 11, 11, "info", "observation"), // inline-only note
            h("a.rs", 30, 30, "info", "test_gap"),    // info but actionable: 8 from #2, 10 from #1
        ];
        // Gaps: (0,2) 8, (2,5) 8, (1,5) 10; (0,5) and (1,2) are 18, past the window.
        assert_eq!(candidate_pairs(&hs), vec![(0, 2), (2, 5), (1, 5)]);
        let many: Vec<_> = (0..10).map(|i| h("c.rs", i, i, "warning", "bug")).collect();
        assert_eq!(candidate_pairs(&many).len(), MAX_PAIRS);
    }

    #[test]
    fn only_same_and_related_calls_become_relations() {
        let a = HighlightResult { path: "a.rs".into(), start_line: 1, end_line: 2, comment: "x".into(), ..Default::default() };
        let j = |rel: &str| PairJudgement { relation: rel.into(), p_same: 0.85, p_related: 0.1, p_different: 0.05, confidence: 0.5 };
        assert_eq!(relation_from(&a, &a, &j("same_issue")).unwrap().relation, "same");
        // A weak "same" only groups: merging would share one verdict.
        let weak = PairJudgement { relation: "same_issue".into(), p_same: 0.62, p_related: 0.35, p_different: 0.03, confidence: 0.4 };
        assert_eq!(relation_from(&a, &a, &weak).unwrap().relation, "related");
        assert_eq!(relation_from(&a, &a, &j("related")).unwrap().relation, "related");
        assert!(relation_from(&a, &a, &j("different")).is_none());
        assert_eq!(relation_from(&a, &a, &j("same_issue")).unwrap().b.comment, "x");
    }

    #[test]
    fn relating_without_a_key_makes_no_calls() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let hs = vec![HighlightResult { path: "a.rs".into(), start_line: 1, end_line: 1, severity: "warning".into(), ..Default::default() }; 2];
        assert!(rt.block_on(relate_findings(None, "t", "b", &hs, &HashMap::new())).is_empty());
    }

    #[test]
    fn questions_serialize_to_typesafes_shape() {
        let v = serde_json::to_value(finding_questions()).unwrap();
        assert_eq!(v["urgency"]["type"], "choice");
        assert!(v["defect"]["criteria"]["real"].is_string());
    }
}
