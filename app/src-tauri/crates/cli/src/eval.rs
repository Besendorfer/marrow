//! `marrow eval` — run the real classification pass over the versioned
//! quality corpus and score precision/recall for RELEVANT (issue #219,
//! roadmap Phase 3). Provider- and model-dependent BY DESIGN: run it before
//! and after a prompt/model change and compare against the same corpus
//! version.

use marrow_core::ai::{extract_json_array, extract_json_object, AiBackend};
use marrow_core::config::{load_settings, resolve_jev_api_key};
use crate::jev_eval::{self, Judged};
use marrow_core::fetch::{
    finalize_coverage, parse_review_response, parse_risk_checks, run_review_pass, validate_classifications, validate_highlights,
};
use marrow_core::repo_tools::{RepoToolTarget, SnapshotRepo, ToolBackend, ToolExecutor, ToolScope};
use marrow_core::prompts::{
    build_classification_prompt, build_highlight_prompt_with, build_requirements_coverage_prompt,
    has_inline_test_markers, is_test_path, risk_check_section, HighlightExtras,
};
use marrow_core::types::{FileClassification, HighlightResult, RequirementsCoverage, ReviewVerdict, RiskCheck, Settings, TopRisk};
use marrow_core::usage::AiUsage;
use std::collections::HashSet;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct FixturePr {
    title: String,
    #[allow(dead_code)]
    body: String,
    files: Vec<FixtureFile>,
}

#[derive(Deserialize)]
struct FixtureFile {
    path: String,
    diff: String,
}

#[derive(Deserialize)]
struct FixtureLabels {
    relevant: Vec<String>,
    not_relevant: Vec<String>,
    /// Regions a good review MUST flag (labels schema v2, issue #221).
    #[serde(default)]
    expected_findings: Vec<LabeledRegion>,
    /// Regions a good review should NOT flag (e.g. the PR's stated purpose).
    #[serde(default)]
    should_not_flag: Vec<LabeledRegion>,
    /// Expected requirements-coverage outcomes (labels schema v3, issue
    /// #229). Substring matching because requirement text is model-extracted.
    #[serde(default)]
    expected_coverage: Vec<ExpectedCoverage>,
    /// Expected review verdict (labels schema v4, issue #231):
    /// "fix_first" | "ship" | "needs_discussion".
    #[serde(default)]
    expected_verdict: Option<String>,
    /// Triage risks handed to the review, each with how it should settle
    /// (labels schema v5, issue #243): "confirmed" (a real defect) or
    /// "cleared" (scary but correct). A false clear is the costly error.
    #[serde(default)]
    risk_checks: Vec<LabeledRisk>,
}

#[derive(Deserialize)]
struct LabeledRisk {
    title: String,
    detail: String,
    path: String,
    #[serde(default)]
    start_line: Option<u64>,
    expected: String,
}

impl LabeledRisk {
    fn as_top_risk(&self) -> TopRisk {
        TopRisk { title: self.title.clone(), detail: self.detail.clone(), path: self.path.clone(), start_line: self.start_line, ai_check: None }
    }
}

const VERDICTS: [&str; 3] = ["fix_first", "ship", "needs_discussion"];

#[derive(Deserialize)]
struct ExpectedCoverage {
    /// Case-insensitive substring that identifies the requirement.
    requirement_contains: String,
    status: String,
}

#[derive(Deserialize)]
struct LabeledRegion {
    path: String,
    start_line: u64,
    end_line: u64,
    #[serde(default = "default_importance")]
    importance: String,
    #[serde(default)]
    #[allow(dead_code)]
    note: String,
}

fn default_importance() -> String {
    "important".to_string()
}

struct FixtureScore {
    name: String,
    true_pos: usize,
    false_pos: usize,
    false_neg: usize,
    mismatches: Vec<String>,
    findings: Option<FindingsScore>,
    coverage: Option<CoverageScore>,
    /// Set when an AI pass failed after retries (issue #226). A failed
    /// classification contributes nothing to the aggregate (its tallies stay
    /// zero); a failed findings pass leaves `findings` at None while the
    /// fixture's classification tallies remain valid and counted.
    failed: Option<String>,
    /// Which pass failed ("classification" | "findings") — drives the
    /// verdict label so a findings-only failure doesn't overstate itself.
    failed_pass: Option<&'static str>,
}

/// Transient provider failures (e.g. the claude CLI's truncated-mid-array
/// responses, ~50% of runs on the adversarial-injection fixture) shouldn't
/// kill a whole eval run. Bounded retries, then the fixture is reported
/// failed and the run continues (issue #226). Config/label validation still
/// fails fast — that's a broken corpus, not a flaky provider.
const PASS_ATTEMPTS: usize = 3;

async fn retry_json_pass<T, F, Fut>(what: &str, name: &str, mut call: F) -> Result<Vec<T>, String>
where
    T: serde::de::DeserializeOwned,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let mut last = String::new();
    for attempt in 1..=PASS_ATTEMPTS {
        match call().await.and_then(|raw| extract_json_array(&raw)) {
            Ok(v) => match serde_json::from_value::<Vec<T>>(v) {
                Ok(parsed) => return Ok(parsed),
                Err(e) => last = format!("unparseable {what}: {e}"),
            },
            Err(e) => last = e,
        }
        if attempt < PASS_ATTEMPTS {
            eprintln!("· {name}: {what} attempt {attempt} failed, retrying…");
        }
    }
    Err(format!("{what} failed after {PASS_ATTEMPTS} attempts: {last}"))
}

/// Review-pass sibling of [`retry_json_pass`] (issue #231): parses with the
/// core's own `parse_review_response` — object with verdict, or legacy
/// array — so the eval can't drift from the app's parser.
async fn retry_review_pass<F, Fut>(name: &str, mut call: F) -> Result<(Vec<HighlightResult>, Option<ReviewVerdict>), String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let mut last = String::new();
    for attempt in 1..=PASS_ATTEMPTS {
        match call().await.and_then(|raw| parse_review_response(&raw)) {
            Ok(parsed) => return Ok(parsed),
            Err(e) => last = e,
        }
        if attempt < PASS_ATTEMPTS {
            eprintln!("· {name}: findings attempt {attempt} failed, retrying…");
        }
    }
    Err(format!("findings failed after {PASS_ATTEMPTS} attempts: {last}"))
}

/// Object-shaped sibling of [`retry_json_pass`] for passes that return a
/// JSON object (the coverage pass), same bounded-retry contract.
async fn retry_json_object_pass<T, F, Fut>(what: &str, name: &str, mut call: F) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let mut last = String::new();
    for attempt in 1..=PASS_ATTEMPTS {
        match call().await.and_then(|raw| extract_json_object(&raw)) {
            Ok(v) => match serde_json::from_value::<T>(v) {
                Ok(parsed) => return Ok(parsed),
                Err(e) => last = format!("unparseable {what}: {e}"),
            },
            Err(e) => last = e,
        }
        if attempt < PASS_ATTEMPTS {
            eprintln!("· {name}: {what} attempt {attempt} failed, retrying…");
        }
    }
    Err(format!("{what} failed after {PASS_ATTEMPTS} attempts: {last}"))
}

/// One agentic review attempt's stats (issue #232).
#[derive(Default)]
struct AgentStats {
    tool_calls: usize,
    degraded: bool,
    degrade_reason: Option<String>,
    repaired: bool,
    reads: Vec<String>,
    /// The accepted answer's raw text (risk checks are parsed from it).
    raw: Option<String>,
}

/// Owner every fixture's PR repo (and its sibling repos) lives under.
const FIXTURE_OWNER: &str = "corpus";

/// Load a fixture's optional `repo/` snapshot (issue #232):
/// `repo/head/**` and `repo/base/**` are the PR repo at its head/base;
/// `repo/other/<name>/**` is sibling repo `<name>` at its default branch.
/// A fixture without `repo/` gets an empty snapshot (tools find nothing).
fn load_snapshot(fixture_dir: &Path, pr_repo: &str) -> Result<SnapshotRepo, String> {
    let mut snap = SnapshotRepo { pr_repo: pr_repo.to_string(), ..Default::default() };
    let root = fixture_dir.join("repo");
    if !root.is_dir() {
        return Ok(snap);
    }
    let mut add_tree = |repo: &str, rev: &str, dir: &Path| -> Result<(), String> {
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
                    let content = fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                    snap.files
                        .entry(repo.to_string())
                        .or_default()
                        .entry(rev.to_string())
                        .or_default()
                        .insert(rel, content);
                }
            }
        }
        Ok(())
    };
    for rev in ["head", "base"] {
        let d = root.join(rev);
        if d.is_dir() {
            add_tree(pr_repo, rev, &d)?;
        }
    }
    let others = root.join("other");
    if others.is_dir() {
        for e in fs::read_dir(&others).map_err(|e| e.to_string())?.flatten() {
            if e.path().is_dir() {
                let name = e.file_name().to_string_lossy().into_owned();
                add_tree(&name, "default", &e.path())?;
            }
        }
    }
    Ok(snap)
}

/// One fixture's work since `start` — every fixture gets one, including
/// those whose classification failed, so per-model averages aren't skewed.
/// Jev time (`--jev`) is excluded; its calls don't go through the AI
/// backend, so the character counts never include them.
fn work_entry(name: &str, start: (u64, u64), started: std::time::Instant, jev_time: std::time::Duration, u0: &AiUsage) -> serde_json::Value {
    let (sent, recv) = marrow_core::ai::traffic::totals();
    let mut entry = serde_json::json!({
        "fixture": name, "chars_sent": sent - start.0, "chars_received": recv - start.1,
        "seconds": started.elapsed().saturating_sub(jev_time).as_secs_f64(),
    });
    for (k, v) in usage_delta(&usage_now(), u0) {
        entry[k] = v;
    }
    entry
}

/// Provider-reported tokens (and cost) between two snapshots of the run's
/// meter — one fixture's share (issue #236). The cost is only known when both
/// snapshots carry one, or the earlier one is the empty start of the run.
fn usage_delta(now: &AiUsage, before: &AiUsage) -> Vec<(&'static str, serde_json::Value)> {
    let cost = match (now.reported_cost_usd, before.reported_cost_usd) {
        (Some(n), Some(b)) => Some(n - b),
        (Some(n), None) if before.calls == 0 => Some(n),
        _ => None,
    };
    vec![
        ("calls", now.calls.saturating_sub(before.calls).into()),
        ("interrupted_calls", now.interrupted_calls.saturating_sub(before.interrupted_calls).into()),
        ("input_tokens", now.input_tokens.saturating_sub(before.input_tokens).into()),
        ("cache_read_tokens", now.cache_read_tokens.saturating_sub(before.cache_read_tokens).into()),
        ("cache_write_tokens", now.cache_write_tokens.saturating_sub(before.cache_write_tokens).into()),
        ("output_tokens", now.output_tokens.saturating_sub(before.output_tokens).into()),
        ("cut_short_input_tokens", now.cut_short_usage.input_tokens.saturating_sub(before.cut_short_usage.input_tokens).into()),
        (
            "cut_short_cache_read_tokens",
            now.cut_short_usage.cache_read_tokens.saturating_sub(before.cut_short_usage.cache_read_tokens).into(),
        ),
        ("reported_cost_usd", serde_json::json!(cost)),
    ]
}

fn short(s: &str) -> String {
    let t: String = s.chars().take(90).collect();
    if s.chars().count() > 90 { format!("{t}…") } else { t }
}


/// A highlight's corpus label: the first expected region it overlaps (by
/// importance), else a should-not-flag region, else unlabeled.
fn label_for(h: &HighlightResult, labels: &FixtureLabels) -> &'static str {
    if let Some(l) = labels.expected_findings.iter().find(|l| overlaps(h.start_line, h.end_line, l, &h.path)) {
        return if l.importance == "minor" { "minor" } else { "important" };
    }
    if labels.should_not_flag.iter().any(|l| overlaps(h.start_line, h.end_line, l, &h.path)) {
        return "should_not_flag";
    }
    "unlabeled"
}

pub async fn eval(corpus: &Path, json: bool, single_shot: bool, jev: bool, model: Option<String>) -> Result<(), String> {
    // A per-run model override (model comparison); the config file is untouched.
    let mut settings = load_settings();
    if let Some(m) = model {
        settings.model = m;
    }
    // Meter the whole run (issue #236): token usage, cache reads and cost,
    // per fixture and in total — what a prompt-caching change is measured by.
    let connection = marrow_core::ai::provider_for_settings(&settings).label();
    let model = settings.model.clone();
    let (out, _) = marrow_core::usage::metered(connection, &model, eval_metered(corpus, json, single_shot, jev, settings)).await;
    out
}

/// The current run's usage so far (a zeroed value outside a metered scope).
fn usage_now() -> AiUsage {
    marrow_core::usage::current().unwrap_or_default()
}

/// One line on what the run's AI calls used and cost.
fn usage_line(u: &AiUsage) -> String {
    let cost = match (u.reported_cost_usd, u.list_cost_usd) {
        (Some(c), _) => format!("${c:.2} reported"),
        (None, Some(c)) => format!("≈${c:.2} at list price"),
        (None, None) => "cost not reported".to_string(),
    };
    let mut line = format!(
        "AI USAGE {} calls via {} · input {} · cache read {} · cache write {} · output {} · {cost}",
        u.calls, u.connection, u.input_tokens, u.cache_read_tokens, u.cache_write_tokens, u.output_tokens
    );
    if u.failed_calls > 0 {
        line.push_str(&format!(" · {} failed call(s) not included", u.failed_calls));
    }
    if u.interrupted_calls > 0 {
        // Tool turns: priced nowhere (their output is never reported), but
        // their cache reads are where prompt caching pays off.
        let c = &u.cut_short_usage;
        line.push_str(&format!(
            " · {} cut-short call(s) not included (before the cut: input {} · cache read {} · cache write {})",
            u.interrupted_calls, c.input_tokens, c.cache_read_tokens, c.cache_write_tokens
        ));
    }
    line
}

async fn eval_metered(corpus: &Path, json: bool, single_shot: bool, jev: bool, settings: Settings) -> Result<(), String> {
    let version = fs::read_to_string(corpus.join("VERSION"))
        .map(|v| v.trim().to_string())
        .map_err(|_| format!("{} does not look like a corpus (no VERSION file)", corpus.display()))?;
    let fixtures_dir = corpus.join("fixtures");
    let mut fixture_dirs: Vec<PathBuf> = fs::read_dir(&fixtures_dir)
        .map_err(|e| format!("Failed to read {}: {e}", fixtures_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    fixture_dirs.sort();
    if fixture_dirs.is_empty() {
        return Err("corpus has no fixtures".to_string());
    }

    // Load and validate EVERY fixture before the first AI call — a bad
    // fixture or a signal-less corpus should fail fast, not mid-spend.
    let mut fixtures: Vec<(String, FixturePr, FixtureLabels, SnapshotRepo)> = Vec::new();
    for dir in &fixture_dirs {
        let name = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let pr: FixturePr = read_json(&dir.join("pr.json"))?;
        let labels: FixtureLabels = read_json(&dir.join("labels.json"))?;
        validate_labels(&pr, &labels, &name)?;
        let snapshot = load_snapshot(dir, &name)?;
        fixtures.push((name, pr, labels, snapshot));
    }
    // Measurement honesty: with zero RELEVANT labels there is nothing to
    // measure — 1.00/1.00 on an empty corpus would be vacuous, not perfect.
    if fixtures.iter().all(|(_, _, l, _)| l.relevant.is_empty()) {
        return Err("corpus has no RELEVANT labels — nothing to measure".to_string());
    }

    // Fail before any spend when --jev can't run.
    let jev_key = if jev {
        Some(resolve_jev_api_key(&settings).ok_or(
            "--jev needs a TypeSafe API key: set it in Settings or TYPESAFE_API_KEY",
        )?)
    } else {
        None
    };
    let mut judged: Vec<Judged> = Vec::new();
    let mut work: Vec<serde_json::Value> = Vec::new();
    // The production relate_findings on each fixture's real review output.
    let mut relations: Vec<(String, marrow_core::types::FindingRelation)> = Vec::new();
    let mut candidate_total = 0usize;
    let ai = AiBackend::from_settings(&settings).await?;
    eprintln!(
        "corpus v{version} · {} fixture(s) · model {} · review {}",
        fixtures.len(),
        if settings.model.is_empty() { "(claude CLI default)" } else { &settings.model },
        if single_shot { "single-shot" } else { "agentic" }
    );

    let mut scores: Vec<FixtureScore> = Vec::new();
    for (name, pr, labels, snapshot) in fixtures {
        let file_list: Vec<String> = pr.files.iter().map(|f| f.path.clone()).collect();
        let full_diff = assemble_full_diff(&pr.files);

        let mut score = FixtureScore { name, true_pos: 0, false_pos: 0, false_neg: 0, mismatches: Vec::new(), findings: None, coverage: None, failed: None, failed_pass: None };
        // How much work this fixture took (model comparison): AI characters
        // exchanged and wall-clock time.
        let (sent0, recv0) = marrow_core::ai::traffic::totals();
        let usage0 = usage_now();
        let started = std::time::Instant::now();
        // Time spent on --jev (its calls and their spacing) is left out of
        // the fixture's seconds, which measure the review model.
        let mut jev_time = std::time::Duration::ZERO;

        let (prompt, _truncated) = build_classification_prompt(&pr.title, &file_list, &full_diff);
        eprintln!("· {}: classifying {} files…", score.name, file_list.len());
        let parsed: Vec<FileClassification> =
            match retry_json_pass("classification", &score.name, || ai.invoke(&prompt)).await {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("· {}: {e}", score.name);
                    score.failed = Some(e);
                    score.failed_pass = Some("classification");
                    work.push(work_entry(&score.name, (sent0, recv0), started, jev_time, &usage0));
                    scores.push(score);
                    continue;
                }
            };
        // Judge POST-validation output — the same pipeline the app runs.
        let validated = validate_classifications(parsed, &file_list);

        let predicted_relevant: std::collections::HashSet<&str> = validated
            .iter()
            .filter(|c| c.classification == "RELEVANT")
            .map(|c| c.path.as_str())
            .collect();
        for p in &labels.relevant {
            if predicted_relevant.contains(p.as_str()) {
                score.true_pos += 1;
            } else {
                score.false_neg += 1;
                score.mismatches.push(format!("{p}: labeled RELEVANT, judged not"));
            }
        }
        for p in &labels.not_relevant {
            if predicted_relevant.contains(p.as_str()) {
                score.false_pos += 1;
                score.mismatches.push(format!("{p}: labeled NOT_RELEVANT, judged relevant"));
            }
        }

        // Findings scoring (issue #221): only for fixtures that carry
        // findings labels. The highlights pass gets the LABEL-relevant
        // files' diffs — ground truth, so classification quality can't
        // contaminate findings quality.
        if !labels.expected_findings.is_empty()
            || !labels.should_not_flag.is_empty()
            || labels.expected_verdict.is_some()
            || !labels.risk_checks.is_empty()
        {
            let risks: Vec<TopRisk> = labels.risk_checks.iter().map(LabeledRisk::as_top_risk).collect();
            let relevant_diffs = label_relevant_diffs(&pr.files, &labels.relevant);
            // Test-file diffs ride along as context, exactly as the app feeds
            // them (issue #231) — detected by the core's own is_test_path.
            let (test_diffs, _) = split_coverage_inputs(&pr.files);
            let (hl_prompt, _t) = build_highlight_prompt_with(
                &pr.title,
                &pr.body,
                &relevant_diffs,
                &[],
                &HighlightExtras { checks: &[], test_diffs: &test_diffs, repo_tools: false },
            );
            let (agentic_prompt, _t) = build_highlight_prompt_with(
                &pr.title,
                &pr.body,
                &relevant_diffs,
                &[],
                &HighlightExtras { checks: &[], test_diffs: &test_diffs, repo_tools: true },
            );
            eprintln!("· {}: reviewing for findings…", score.name);
            // The last attempt's agent stats (tool calls, degraded, reads).
            let stats: std::sync::Mutex<AgentStats> = std::sync::Mutex::new(AgentStats::default());
            let result = if single_shot {
                let prompt = format!("{hl_prompt}{}", risk_check_section(&risks, false));
                retry_review_pass(&score.name, || async {
                    let out = ai.invoke(&prompt).await;
                    stats.lock().unwrap().raw = out.as_ref().ok().cloned();
                    out
                })
                .await
            } else {
                retry_review_pass(&score.name, || async {
                    let ex = ToolExecutor::new(
                        ToolBackend::Snapshot(&snapshot),
                        RepoToolTarget {
                            owner: FIXTURE_OWNER.to_string(),
                            repo: score.name.clone(),
                            head_sha: "head".to_string(),
                            base_sha: "base".to_string(),
                        },
                        ToolScope::REVIEW,
                    );
                    let out = run_review_pass(&ai, &ex, &agentic_prompt, &hl_prompt, &risks).await;
                    let reads = out.reads.iter().map(|r| format!("{} {} {} {}", r.tool, r.repo, r.rev, r.path)).collect();
                    *stats.lock().unwrap() = AgentStats {
                        tool_calls: out.tool_calls,
                        degraded: out.degraded,
                        degrade_reason: out.degrade_reason.clone(),
                        repaired: out.repaired,
                        reads,
                        raw: out.raw.as_ref().ok().cloned(),
                    };
                    out.raw
                })
                .await
            };
            match result {
                Ok((parsed, verdict)) => {
                    // Findings anchored outside the PR are dropped by
                    // validation; count them so a verdict with nothing
                    // behind it can't hide (issue #232).
                    let out_of_diff: Vec<String> = parsed
                        .iter()
                        .filter(|h| !file_list.contains(&h.path))
                        .map(|h| format!("OUT-OF-DIFF dropped: {} L{}-{} ({})", h.path, h.start_line, h.end_line, h.category))
                        .collect();
                    let validated = validate_highlights(parsed, &file_list);
                    let jev_started = std::time::Instant::now();
                    if let Some(key) = &jev_key {
                        eprintln!("· {}: asking Jev about {} finding(s)…", score.name, validated.len());
                        for h in &validated {
                            let diff = pr.files.iter().find(|f| f.path == h.path).map(|f| f.diff.as_str()).unwrap_or("");
                            let judgement = marrow_core::jev::judge_finding(key, &pr.title, &pr.body, h, diff).await;
                            judged.push(Judged {
                                fixture: score.name.clone(),
                                path: h.path.clone(),
                                lines: format!("{}-{}", h.start_line, h.end_line),
                                category: h.category.clone(),
                                label: label_for(h, &labels),
                                judgement,
                            });
                            tokio::time::sleep(marrow_core::jev::CALL_SPACING).await;
                        }
                        let diffs: std::collections::HashMap<String, String> =
                            pr.files.iter().map(|f| (f.path.clone(), f.diff.clone())).collect();
                        let candidates = marrow_core::jev::candidate_pairs(&validated).len();
                        let stored = marrow_core::jev::relate_findings(Some(key), &pr.title, &pr.body, &validated, &diffs).await;
                        // relate_findings drops failed or timed-out calls along
                        // with "different" ones; say how many pairs were asked.
                        eprintln!("· {}: {candidates} candidate pair(s) → {} relation(s) stored", score.name, stored.len());
                        candidate_total += candidates;
                        for r in stored {
                            relations.push((score.name.clone(), r));
                        }
                    }
                    jev_time += jev_started.elapsed();
                    let mut fs = score_findings(&validated, &labels);
                    fs.out_of_diff = out_of_diff.len();
                    fs.detail.extend(out_of_diff);
                    score_verdict(&mut fs, verdict.as_ref(), labels.expected_verdict.as_deref());
                    let st = stats.into_inner().unwrap();
                    let checks = st.raw.as_deref().map(|raw| parse_risk_checks(raw, risks.len())).unwrap_or_default();
                    fs.risks = score_risk_checks(&labels.risk_checks, &checks, &validated);
                    fs.tool_calls = st.tool_calls;
                    fs.degraded = st.degraded;
                    fs.degrade_reason = st.degrade_reason;
                    fs.repaired = st.repaired;
                    fs.reads = st.reads;
                    score.findings = Some(fs);
                }
                Err(e) => {
                    eprintln!("· {}: {e}", score.name);
                    score.failed = Some(e);
                    score.failed_pass = Some("findings");
                }
            }
        }

        // Requirements-coverage scoring (issue #229): only for fixtures that
        // carry coverage expectations. Files are split by the core's own
        // test detectors; hallucination is counted on the RAW parse, status
        // accuracy on the POST-finalize output — the pipeline the app runs.
        if !labels.expected_coverage.is_empty() && score.failed.is_none() {
            let (test_diffs, inline_test_diffs) = split_coverage_inputs(&pr.files);
            let (cov_prompt, _t) = build_requirements_coverage_prompt(
                &pr.title,
                &pr.body,
                &test_diffs,
                &inline_test_diffs,
                &[],
                &file_list,
                None,
                &[],
            );
            eprintln!("· {}: judging requirements coverage…", score.name);
            match retry_json_object_pass::<RequirementsCoverage, _, _>("coverage", &score.name, || ai.invoke(&cov_prompt)).await {
                Ok(raw_cov) => {
                    let known: HashSet<&str> = test_diffs
                        .iter()
                        .chain(inline_test_diffs.iter())
                        .map(|(p, _)| p.as_str())
                        .collect();
                    let (hallucinated, mut hdetail) = count_hallucinated_citations(&raw_cov, &known);
                    let finalized = finalize_coverage(raw_cov, &known);
                    let mut cs = score_coverage(finalized.as_ref(), &labels.expected_coverage);
                    cs.hallucinated = hallucinated;
                    cs.detail.append(&mut hdetail);
                    score.coverage = Some(cs);
                }
                Err(e) => {
                    eprintln!("· {}: {e}", score.name);
                    score.failed = Some(e);
                    score.failed_pass = Some("coverage");
                }
            }
        }
        work.push(work_entry(&score.name, (sent0, recv0), started, jev_time, &usage0));
        scores.push(score);
    }

    let (tp, fp, fneg) = scores.iter().fold((0, 0, 0), |(a, b, c), s| {
        (a + s.true_pos, b + s.false_pos, c + s.false_neg)
    });
    let precision = ratio(tp, tp + fp);
    let recall = ratio(tp, tp + fneg);

    let jev_summary = jev.then(|| jev_eval::summarize(&judged));
    if json {
        let mut out = render_json_report(&scores, &version, &settings.model, precision, recall);
        out["work"] = serde_json::json!(work);
        out["usage"] = serde_json::to_value(usage_now()).unwrap_or_default();
        if let Some(s) = &jev_summary {
            out["jev"] = serde_json::json!({ "summary": s, "judged": judged, "relations": relations });
        }
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        print!("{}", render_text_report(&scores, &version, precision, recall));
        println!("{}", usage_line(&usage_now()));
        if let Some(s) = &jev_summary {
            print!("{}", jev_eval::render_text(s));
            println!("JEV RELATIONS stored ({} of {} candidate pairs; the rest judged different, or failed):", relations.len(), candidate_total);
            for (fx, r) in &relations {
                println!(
                    "    {fx} {}: {}:{}-{} [{}] ↔ {}:{}-{} · same {:.2} related {:.2}",
                    r.relation, r.a.path, r.a.start_line, r.a.end_line, short(&r.a.comment), r.b.path, r.b.start_line, r.b.end_line, r.p_same, r.p_related
                );
                println!("        b: {}", short(&r.b.comment));
            }
        }
    }
    completion_status(&scores)
}

/// `eval --jev-classify` (issue #249): classify every corpus file with the
/// LLM pass and with Jev, both through validate_classifications, and score
/// each against the labels. No findings or coverage passes.
pub async fn eval_jev_classify(corpus: &Path, json: bool, model: Option<String>) -> Result<(), String> {
    use crate::jev_classify::{render_text, summarize, Row};
    use std::time::Instant;
    let mut settings = load_settings();
    if let Some(m) = model {
        settings.model = m;
    }
    let key = resolve_jev_api_key(&settings).ok_or("--jev-classify needs a TypeSafe API key: set it in Settings or TYPESAFE_API_KEY")?;
    let fixtures_dir = corpus.join("fixtures");
    let mut dirs: Vec<PathBuf> = fs::read_dir(&fixtures_dir)
        .map_err(|e| format!("Failed to read {}: {e}", fixtures_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    let mut fixtures = Vec::new();
    for dir in &dirs {
        let name = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let pr: FixturePr = read_json(&dir.join("pr.json"))?;
        let labels: FixtureLabels = read_json(&dir.join("labels.json"))?;
        validate_labels(&pr, &labels, &name)?;
        fixtures.push((name, pr, labels));
    }
    let ai = AiBackend::from_settings(&settings).await?;
    eprintln!("{} fixture(s) · LLM {} vs Jev", fixtures.len(), if settings.model.is_empty() { "(claude CLI default)" } else { &settings.model });

    let (mut rows, mut jev_calls, mut llm_passes) = (Vec::new(), Vec::new(), Vec::new());
    for (name, pr, labels) in &fixtures {
        let file_list: Vec<String> = pr.files.iter().map(|f| f.path.clone()).collect();
        let (prompt, _) = build_classification_prompt(&pr.title, &file_list, &assemble_full_diff(&pr.files));
        let started = Instant::now();
        let llm = retry_json_pass::<FileClassification, _, _>("classification", name, || ai.invoke(&prompt)).await;
        llm_passes.push(started.elapsed());
        let llm = llm.map(|c| validate_classifications(c, &file_list)).unwrap_or_else(|e| {
            eprintln!("· {name}: LLM {e}");
            Vec::new()
        });
        for f in &pr.files {
            let label = labels.relevant.contains(&f.path);
            let llm_c = llm.iter().find(|c| c.path == f.path);
            let started = Instant::now();
            let jev = marrow_core::jev::classify_file(&key, &pr.title, &pr.body, &f.path, &file_list, &f.diff).await;
            jev_calls.push(started.elapsed());
            let (p_jev, jev_risk, error) = match jev {
                Ok(j) => {
                    // Same validation the app applies to the LLM's output.
                    let v = validate_classifications(vec![j.to_classification(&f.path)], &file_list);
                    let risk = v.first().map(|c| c.risk_level.clone());
                    (Some(j.p_relevant), risk, None)
                }
                Err(e) => (None, None, Some(e)),
            };
            rows.push(Row {
                source: name.clone(),
                path: f.path.clone(),
                label,
                llm: llm_c.map(|c| c.classification == "RELEVANT"),
                p_jev,
                llm_risk: llm_c.map(|c| c.risk_level.clone()),
                jev_risk,
                error,
            });
            eprint!(".");
            tokio::time::sleep(marrow_core::jev::CALL_SPACING).await;
        }
        eprintln!(" {name}");
    }
    let s = summarize(&rows, &jev_calls, &llm_passes);
    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "summary": s, "rows": rows })).unwrap());
    } else {
        print!("{}", render_text(&format!("FILE RELEVANCE · corpus · LLM vs Jev · {} files", rows.len()), &s, &rows));
    }
    Ok(())
}

/// Exit-status contract: the run always completes and emits its full report,
/// but any fixture with a failed pass makes the process exit nonzero so CI
/// can detect it without parsing output (grep-style: results AND status).
fn completion_status(scores: &[FixtureScore]) -> Result<(), String> {
    let failed = scores.iter().filter(|s| s.failed.is_some()).count();
    if failed > 0 {
        Err(format!("{failed} of {} fixture(s) had a failed pass — see report above", scores.len()))
    } else {
        Ok(())
    }
}

/// Render the JSON report. Pure for the same reason as
/// [`render_text_report`]: the failed/failed_pass outcome fields are part of
/// the reporting contract (issue #226) and must stay testable offline.
fn render_json_report(scores: &[FixtureScore], version: &str, model: &str, precision: f64, recall: f64) -> serde_json::Value {
    let (vm, vl, c, n) = review_totals(scores);
    let r = risk_totals(scores);
    let review_json = serde_json::json!({
        "verdict_matched": vm, "verdict_labeled": vl, "complete": c, "substantive": n,
        "risk_checks": { "labeled": r.labeled, "correct": r.correct, "false_clears": r.false_clears,
                         "false_confirms": r.false_confirms, "unresolved": r.unresolved, "unanswered": r.unanswered,
                         "confirmed_unanchored": r.confirmed_unanchored },
    });
    serde_json::json!({
        "corpus_version": version,
        "model": model,
        "fixtures": scores.iter().map(|s| serde_json::json!({
            "name": s.name,
            "true_pos": s.true_pos, "false_pos": s.false_pos, "false_neg": s.false_neg,
            "mismatches": s.mismatches,
            "failed": s.failed,
            "failed_pass": s.failed_pass,
            "findings": s.findings.as_ref().map(|f| serde_json::json!({
                "important_found": f.important_found, "important_missed": f.important_missed,
                "minor_found": f.minor_found, "minor_missed": f.minor_missed,
                "low_value": f.low_value, "extra": f.extra,
                "substantive": f.substantive, "complete": f.complete,
                "verdict": f.verdict, "verdict_match": f.verdict_match, "shapes": f.shapes,
                "tool_calls": f.tool_calls, "degraded": f.degraded, "degrade_reason": f.degrade_reason,
                "repaired": f.repaired, "reads": f.reads, "out_of_diff": f.out_of_diff,
                "risks": { "labeled": f.risks.labeled, "correct": f.risks.correct, "false_clears": f.risks.false_clears,
                           "false_confirms": f.risks.false_confirms, "unresolved": f.risks.unresolved,
                           "unanswered": f.risks.unanswered, "confirmed_unanchored": f.risks.confirmed_unanchored,
                           "detail": f.risks.detail },
                "detail": f.detail,
            })),
            "coverage": s.coverage.as_ref().map(|c| serde_json::json!({
                "status_match": c.status_match, "status_mismatch": c.status_mismatch,
                "not_extracted": c.not_extracted, "extra": c.extra,
                "hallucinated": c.hallucinated, "detail": c.detail,
            })),
        })).collect::<Vec<_>>(),
        "precision": precision,
        "recall": recall,
        "review": review_json,
    })
}

/// Render the human-readable report. Pure so the reporting contract —
/// per-fixture verdicts (a findings-only failure must not overstate itself),
/// FAILED detail lines, and the incomplete-coverage warning — is testable
/// without an AI backend (issue #226).
fn render_text_report(scores: &[FixtureScore], version: &str, precision: f64, recall: f64) -> String {
    use std::fmt::Write;
    let mut out = String::from("\n");
    for s in scores {
        let verdict = match s.failed_pass {
            Some("classification") => "FAILED (classification)".to_string(),
            Some(pass) => {
                let class_verdict = if s.mismatches.is_empty() { "clean" } else { "MISMATCHES" };
                format!("{class_verdict} · {pass} FAILED")
            }
            None if s.mismatches.is_empty() => "clean".to_string(),
            None => "MISMATCHES".to_string(),
        };
        let _ = writeln!(out, "{:<24} tp={} fp={} fn={}  {}", s.name, s.true_pos, s.false_pos, s.false_neg, verdict);
        if let Some(e) = &s.failed {
            let _ = writeln!(out, "    {e}");
        }
        for m in &s.mismatches {
            let _ = writeln!(out, "    {m}");
        }
        if let Some(f) = &s.findings {
            let _ = writeln!(
                out,
                "{:<24} findings: important {}/{} · minor {}/{} · low-value {} · extra {}",
                "",
                f.important_found,
                f.important_found + f.important_missed,
                f.minor_found,
                f.minor_found + f.minor_missed,
                f.low_value,
                f.extra
            );
            let verdict = match (&f.verdict, f.verdict_match) {
                (v, Some(true)) => format!("{} ✓", v.as_deref().unwrap_or("none")),
                (v, Some(false)) => format!("{} ✗", v.as_deref().unwrap_or("none")),
                (v, None) => format!("{} (unlabeled)", v.as_deref().unwrap_or("none")),
            };
            let _ = writeln!(
                out,
                "{:<24} review: verdict {verdict} · complete {}/{} · tools {} · out-of-diff {}{}",
                "",
                f.complete,
                f.substantive,
                f.tool_calls,
                f.out_of_diff,
                if f.degraded { " · DEGRADED (single-shot fallback)" } else if f.repaired { " · repaired" } else { "" }
            );
            if let Some(r) = &f.degrade_reason {
                let _ = writeln!(out, "    degraded: {r}");
            }
            if f.risks.labeled > 0 {
                let r = &f.risks;
                let _ = writeln!(
                    out,
                    "{:<24} risks: correct {}/{} · false clears {} · false confirms {} · unresolved {} · unanswered {} · confirmed w/o finding {}",
                    "", r.correct, r.labeled, r.false_clears, r.false_confirms, r.unresolved, r.unanswered, r.confirmed_unanchored
                );
                for d in &r.detail {
                    let _ = writeln!(out, "    {d}");
                }
            }
            for d in &f.detail {
                let _ = writeln!(out, "    {d}");
            }
        }
        if let Some(c) = &s.coverage {
            let _ = writeln!(
                out,
                "{:<24} coverage: status {}/{} · not-extracted {} · extra {} · hallucinated {}",
                "",
                c.status_match,
                c.status_match + c.status_mismatch + c.not_extracted,
                c.not_extracted,
                c.extra,
                c.hallucinated
            );
            for d in &c.detail {
                let _ = writeln!(out, "    {d}");
            }
        }
    }
    out.push('\n');
    let failed = scores.iter().filter(|s| s.failed.is_some()).count();
    if failed > 0 {
        let _ = writeln!(
            out,
            "⚠ {failed} of {} fixture(s) had a failed pass after {PASS_ATTEMPTS} attempts — numbers below cover completed passes only",
            scores.len()
        );
    }
    let _ = writeln!(out, "RELEVANT precision {precision:.2} · recall {recall:.2} (corpus v{version})");
    let (vm, vl, comp, subst) = review_totals(scores);
    let _ = writeln!(out, "REVIEW verdict {vm}/{vl} · complete findings {comp}/{subst}");
    let r = risk_totals(scores);
    if r.labeled > 0 {
        let _ = writeln!(
            out,
            "RISK CHECKS correct {}/{} · false clears {} · false confirms {} · unresolved {} · unanswered {} · confirmed w/o finding {}",
            r.correct, r.labeled, r.false_clears, r.false_confirms, r.unresolved, r.unanswered, r.confirmed_unanchored
        );
    }
    out
}

/// Risk-check totals across fixtures (issue #243).
fn risk_totals(scores: &[FixtureScore]) -> RiskScore {
    let mut t = RiskScore::default();
    for r in scores.iter().filter_map(|s| s.findings.as_ref()).map(|f| &f.risks) {
        t.labeled += r.labeled;
        t.correct += r.correct;
        t.false_clears += r.false_clears;
        t.false_confirms += r.false_confirms;
        t.unresolved += r.unresolved;
        t.unanswered += r.unanswered;
        t.confirmed_unanchored += r.confirmed_unanchored;
    }
    t
}

/// Aggregate review metrics (issue #231): (verdicts matched, verdicts
/// labeled, complete findings, substantive findings).
fn review_totals(scores: &[FixtureScore]) -> (usize, usize, usize, usize) {
    scores.iter().filter_map(|s| s.findings.as_ref()).fold((0, 0, 0, 0), |(vm, vl, c, n), f| {
        (
            vm + usize::from(f.verdict_match == Some(true)),
            vl + usize::from(f.verdict_match.is_some()),
            c + f.complete,
            n + f.substantive,
        )
    })
}

/// Findings scorecard for one fixture (issue #221).
#[derive(Default)]
struct FindingsScore {
    important_found: usize,
    important_missed: usize,
    minor_found: usize,
    minor_missed: usize,
    low_value: usize,
    extra: usize,
    /// bug/behavior/test_gap findings (issue #231) — the ones that owe a
    /// scenario and a fix.
    substantive: usize,
    /// …of which carry both a non-empty scenario and fix.
    complete: usize,
    /// The model's verdict (None = absent/unknown).
    verdict: Option<String>,
    /// Some(matched?) when the fixture labels an expected verdict.
    verdict_match: Option<bool>,
    /// One "path L{s}-{e} severity/category" line per finding — JSON-only
    /// diagnostic for tuning category assignment.
    shapes: Vec<String>,
    /// Agentic review (issue #232): tool calls attempted, whether it fell
    /// back to single-shot, and what it read ("tool repo rev path").
    tool_calls: usize,
    degraded: bool,
    degrade_reason: Option<String>,
    repaired: bool,
    reads: Vec<String>,
    /// Findings discarded for anchoring on a file outside the PR.
    out_of_diff: usize,
    /// Triage-risk checks (issue #243).
    risks: RiskScore,
    detail: Vec<String>,
}

/// How the review settled the fixture's labeled triage risks (issue #243).
#[derive(Default, Debug, PartialEq)]
struct RiskScore {
    labeled: usize,
    /// Answered with the labeled outcome.
    correct: usize,
    /// A real defect answered "cleared" — the costly error.
    false_clears: usize,
    /// A correct-but-scary change answered "confirmed".
    false_confirms: usize,
    unresolved: usize,
    /// No usable answer for the risk.
    unanswered: usize,
    /// Confirmed with no finding near the risk's line — the defect would show
    /// twice in the app (the risk and a separate note) or only as the risk.
    confirmed_unanchored: usize,
    detail: Vec<String>,
}

/// Lines within which a finding counts as the one confirming a risk — the
/// app's risk/note merge window (findings.ts MERGE_WINDOW).
const RISK_MERGE_WINDOW: u64 = 10;

/// Score the review's answers against the labels, by position (the risks
/// were handed over in label order).
fn score_risk_checks(labels: &[LabeledRisk], checks: &[Option<RiskCheck>], findings: &[HighlightResult]) -> RiskScore {
    let mut s = RiskScore { labeled: labels.len(), ..Default::default() };
    for (i, label) in labels.iter().enumerate() {
        let got = checks.get(i).cloned().flatten();
        let outcome = got.as_ref().map(|c| c.outcome.as_str());
        let anchored = findings.iter().any(|h| {
            h.path == label.path
                && label.start_line.map_or(true, |l| {
                    l + RISK_MERGE_WINDOW >= h.start_line && l <= h.end_line + RISK_MERGE_WINDOW
                })
        });
        let unanchored = outcome == Some("confirmed") && !anchored;
        if unanchored {
            s.confirmed_unanchored += 1;
        }
        match outcome {
            None => s.unanswered += 1,
            Some("unresolved") => s.unresolved += 1,
            Some(o) if o == label.expected => s.correct += 1,
            Some("cleared") => s.false_clears += 1,
            Some(_) => s.false_confirms += 1,
        }
        let mark = match outcome {
            Some(o) if o == label.expected => "✓",
            None | Some("unresolved") => "·",
            _ => "✗",
        };
        s.detail.push(format!(
            "RISK {mark} {} (expected {}, got {}){}",
            label.title,
            label.expected,
            outcome.unwrap_or("no answer"),
            got.as_ref().map(|c| if c.reason.is_empty() { String::new() } else { format!(": {}", c.reason) }).unwrap_or_default()
        ));
        if unanchored {
            s.detail.push(format!("    ↳ no finding near {}:{}", label.path, label.start_line.map(|l| l.to_string()).unwrap_or_default()));
        }
    }
    s
}

/// Requirements-coverage scorecard for one fixture (issue #229).
#[derive(Default)]
struct CoverageScore {
    /// Expected requirements matched with the expected status.
    status_match: usize,
    /// Matched a requirement, wrong status.
    status_mismatch: usize,
    /// No extracted requirement contained the expected substring.
    not_extracted: usize,
    /// Extracted requirements matching no expectation (neutral).
    extra: usize,
    /// Test paths cited in the RAW parse that were never shown to the model.
    hallucinated: usize,
    detail: Vec<String>,
}

/// Score post-finalize coverage output against expectations. `None` coverage
/// (finalize dropped everything) scores every expectation as not-extracted.
fn score_coverage(cov: Option<&RequirementsCoverage>, expected: &[ExpectedCoverage]) -> CoverageScore {
    let mut s = CoverageScore::default();
    let reqs: &[marrow_core::types::RequirementEntry] =
        cov.map(|c| c.requirements.as_slice()).unwrap_or(&[]);
    let mut matched_req = vec![false; reqs.len()];
    for exp in expected {
        let needle = exp.requirement_contains.to_lowercase();
        match reqs.iter().position(|r| r.text.to_lowercase().contains(&needle)) {
            Some(i) => {
                matched_req[i] = true;
                if reqs[i].status == exp.status {
                    s.status_match += 1;
                } else {
                    s.status_mismatch += 1;
                    s.detail.push(format!(
                        "\"{}\": expected {}, got {}",
                        exp.requirement_contains, exp.status, reqs[i].status
                    ));
                }
            }
            None => {
                s.not_extracted += 1;
                s.detail.push(format!("\"{}\": no requirement extracted", exp.requirement_contains));
            }
        }
    }
    s.extra = matched_req.iter().filter(|m| !**m).count();
    s
}

/// Split fixture files the way the app feeds the coverage pass: test files
/// by path convention, plus implementation diffs that ADD inline tests —
/// using the core's own detectors so the eval can't drift from the pipeline.
fn split_coverage_inputs(files: &[FixtureFile]) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let test_diffs = files
        .iter()
        .filter(|f| is_test_path(&f.path))
        .map(|f| (f.path.clone(), f.diff.clone()))
        .collect();
    let inline_test_diffs = files
        .iter()
        .filter(|f| !is_test_path(&f.path) && has_inline_test_markers(&f.diff))
        .map(|f| (f.path.clone(), f.diff.clone()))
        .collect();
    (test_diffs, inline_test_diffs)
}

/// Count citations in the RAW parsed coverage (pre-finalize) to paths the
/// model was never shown — the hallucinated-evidence measurement.
fn count_hallucinated_citations(cov: &RequirementsCoverage, known: &HashSet<&str>) -> (usize, Vec<String>) {
    let mut detail = Vec::new();
    for t in cov
        .requirements
        .iter()
        .flat_map(|r| r.tests.iter())
        .chain(cov.orphan_tests.iter())
    {
        if !known.contains(t.path.as_str()) {
            detail.push(format!("hallucinated citation: {}", t.path));
        }
    }
    (detail.len(), detail)
}

/// A model highlight matches a labeled region when paths are equal and line
/// ranges overlap.
fn overlaps(h_start: u64, h_end: u64, l: &LabeledRegion, path: &str) -> bool {
    path == l.path && h_start <= l.end_line && h_end >= l.start_line
}

/// Score validated highlights against a fixture's findings labels: each
/// expected region is found or missed (by importance); highlights matching a
/// should_not_flag region are low-value; highlights matching no label are
/// counted neutrally as extra — an unlabeled highlight is not automatically
/// noise.
fn score_findings(
    highlights: &[marrow_core::types::HighlightResult],
    labels: &FixtureLabels,
) -> FindingsScore {
    let mut score = FindingsScore::default();
    for l in &labels.expected_findings {
        let found = highlights.iter().any(|h| overlaps(h.start_line, h.end_line, l, &h.path));
        match (l.importance.as_str(), found) {
            ("minor", true) => score.minor_found += 1,
            ("minor", false) => {
                score.minor_missed += 1;
                score.detail.push(format!("MISSED minor: {} L{}-{}", l.path, l.start_line, l.end_line));
            }
            (_, true) => score.important_found += 1,
            (_, false) => {
                score.important_missed += 1;
                score.detail.push(format!("MISSED important: {} L{}-{}", l.path, l.start_line, l.end_line));
            }
        }
    }
    for h in highlights {
        let expected = labels.expected_findings.iter().any(|l| overlaps(h.start_line, h.end_line, l, &h.path));
        let noise = labels.should_not_flag.iter().any(|l| overlaps(h.start_line, h.end_line, l, &h.path));
        if noise && !expected {
            score.low_value += 1;
            score.detail.push(format!("LOW-VALUE: {} L{}-{} flags a should-not-flag region", h.path, h.start_line, h.end_line));
        } else if !expected {
            score.extra += 1;
        }
        score.shapes.push(format!("{} L{}-{} {}/{}", h.path, h.start_line, h.end_line, h.severity, h.category));
        if matches!(h.category.as_str(), "bug" | "behavior" | "test_gap") {
            score.substantive += 1;
            if !h.scenario.trim().is_empty() && !h.fix.trim().is_empty() {
                score.complete += 1;
            } else {
                score.detail.push(format!(
                    "INCOMPLETE: {} L{}-{} ({}) lacks a scenario or fix",
                    h.path, h.start_line, h.end_line, h.category
                ));
            }
        }
    }
    score
}

/// Record the model's verdict and, when the fixture labels one, whether it
/// matched (issue #231). A missing verdict against a label is a mismatch.
fn score_verdict(score: &mut FindingsScore, got: Option<&ReviewVerdict>, expected: Option<&str>) {
    score.verdict = got.map(|v| v.verdict.clone());
    if let Some(exp) = expected {
        let ok = score.verdict.as_deref() == Some(exp);
        score.verdict_match = Some(ok);
        if !ok {
            score.detail.push(format!(
                "VERDICT: expected {exp}, got {}",
                score.verdict.as_deref().unwrap_or("none")
            ));
        }
    }
}

/// Assemble the whole-PR diff the way GitHub serves it — per-file bodies
/// under `diff --git` headers, with a guaranteed newline between segments so
/// a fixture diff lacking a trailing newline can't abut the next header.
fn assemble_full_diff(files: &[FixtureFile]) -> String {
    let mut out = String::new();
    for f in files {
        out.push_str(&format!("diff --git a/{p} b/{p}\n--- a/{p}\n+++ b/{p}\n", p = f.path));
        out.push_str(&f.diff);
        if !f.diff.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn ratio(num: usize, den: usize) -> f64 {
    if den == 0 { 1.0 } else { num as f64 / den as f64 }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let content = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&content).map_err(|e| format!("{}: {e}", path.display()))
}

/// The findings pass is fed LABEL-relevant diffs — ground truth, never the
/// classification pass's output — so classification quality can't
/// contaminate findings quality.
fn label_relevant_diffs(files: &[FixtureFile], relevant: &[String]) -> Vec<(String, String)> {
    files
        .iter()
        .filter(|f| relevant.contains(&f.path))
        .map(|f| (f.path.clone(), f.diff.clone()))
        .collect()
}

/// Every fixture path must be labeled exactly once — a mislabeled corpus
/// measures nothing.
fn validate_labels(pr: &FixturePr, labels: &FixtureLabels, name: &str) -> Result<(), String> {
    for f in &pr.files {
        let in_rel = labels.relevant.contains(&f.path);
        let in_not = labels.not_relevant.contains(&f.path);
        if in_rel == in_not {
            return Err(format!(
                "{name}: {} must appear in exactly one of relevant/not_relevant",
                f.path
            ));
        }
    }
    let labeled = labels.relevant.len() + labels.not_relevant.len();
    if labeled != pr.files.len() {
        return Err(format!("{name}: {labeled} labels for {} files", pr.files.len()));
    }
    for r in labels.expected_findings.iter().chain(labels.should_not_flag.iter()) {
        if r.importance != "important" && r.importance != "minor" {
            return Err(format!(
                "{name}: unknown importance {:?} on {} (use \"important\" or \"minor\")",
                r.importance, r.path
            ));
        }
        // The highlights pass only ever sees label-RELEVANT diffs, so a
        // findings region on any other path is silently unwinnable (always
        // MISSED) or inert (never flaggable) — it measures nothing.
        if !labels.relevant.contains(&r.path) {
            return Err(format!(
                "{name}: findings region on {} which is not label-relevant — it could never be scored",
                r.path
            ));
        }
    }
    if let Some(v) = &labels.expected_verdict {
        if !VERDICTS.contains(&v.as_str()) {
            return Err(format!("{name}: unknown expected_verdict {v:?} (use one of {VERDICTS:?})"));
        }
    }
    for r in &labels.risk_checks {
        if !matches!(r.expected.as_str(), "confirmed" | "cleared") {
            return Err(format!("{name}: risk \"{}\" expects {:?} (use \"confirmed\" or \"cleared\")", r.title, r.expected));
        }
        if !labels.relevant.contains(&r.path) {
            return Err(format!("{name}: risk \"{}\" is on {} which is not label-relevant", r.title, r.path));
        }
    }
    for e in &labels.expected_coverage {
        if e.requirement_contains.trim().is_empty() {
            return Err(format!("{name}: expected_coverage entry with empty requirement_contains"));
        }
        if !matches!(e.status.as_str(), "covered" | "partial" | "uncovered" | "untestable") {
            return Err(format!(
                "{name}: unknown coverage status {:?} for \"{}\"",
                e.status, e.requirement_contains
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    /// Every committed fixture (including corpus v7's hard ones) loads and
    /// passes the label checks the eval runs before spending on AI calls.
    #[test]
    fn every_corpus_fixture_loads_and_validates() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../corpus");
        let version: u32 = std::fs::read_to_string(dir.join("VERSION")).unwrap().trim().parse().unwrap();
        assert!(version >= 7, "corpus VERSION {version}");
        let mut n = 0;
        for entry in std::fs::read_dir(dir.join("fixtures")).unwrap() {
            let fx = entry.unwrap().path();
            if !fx.is_dir() {
                continue;
            }
            let name = fx.file_name().unwrap().to_string_lossy().into_owned();
            let pr: FixturePr = read_json(&fx.join("pr.json")).unwrap();
            let labels: FixtureLabels = read_json(&fx.join("labels.json")).unwrap();
            validate_labels(&pr, &labels, &name).unwrap_or_else(|e| panic!("{name}: {e}"));
            n += 1;
        }
        assert!(n >= 14, "expected v6's 9 fixtures plus v7's 5, found {n}");
    }
    use super::*;

    #[test]
    fn label_validation_catches_gaps_and_overlaps() {
        let pr = FixturePr {
            title: "t".into(),
            body: "b".into(),
            files: vec![
                FixtureFile { path: "a.rs".into(), diff: String::new() },
                FixtureFile { path: "b.rs".into(), diff: String::new() },
            ],
        };
        let ok = FixtureLabels { relevant: vec!["a.rs".into()], not_relevant: vec!["b.rs".into()], expected_findings: vec![], should_not_flag: vec![], expected_coverage: vec![], expected_verdict: None, risk_checks: vec![] };
        assert!(validate_labels(&pr, &ok, "f").is_ok());
        let overlap = FixtureLabels { relevant: vec!["a.rs".into(), "b.rs".into()], not_relevant: vec!["b.rs".into()], expected_findings: vec![], should_not_flag: vec![], expected_coverage: vec![], expected_verdict: None, risk_checks: vec![] };
        assert!(validate_labels(&pr, &overlap, "f").is_err());
        let missing = FixtureLabels { relevant: vec!["a.rs".into()], not_relevant: vec![], expected_findings: vec![], should_not_flag: vec![], expected_coverage: vec![], expected_verdict: None, risk_checks: vec![] };
        assert!(validate_labels(&pr, &missing, "f").is_err());
        // A typo'd importance must not silently bucket as "important".
        let typo = FixtureLabels {
            relevant: vec!["a.rs".into()],
            not_relevant: vec!["b.rs".into()],
            expected_findings: vec![region("a.rs", 1, 2, "importnat")],
            should_not_flag: vec![],
            expected_coverage: vec![],
            expected_verdict: None,
            risk_checks: vec![],
        };
        assert!(validate_labels(&pr, &typo, "f").is_err());
        // A findings region on a non-relevant path could never be scored.
        let unwinnable = FixtureLabels {
            relevant: vec!["a.rs".into()],
            not_relevant: vec!["b.rs".into()],
            expected_findings: vec![region("b.rs", 1, 2, "important")],
            should_not_flag: vec![],
            expected_coverage: vec![],
            expected_verdict: None,
            risk_checks: vec![],
        };
        assert!(validate_labels(&pr, &unwinnable, "f").is_err());
    }

    #[test]
    fn ratios_handle_empty_denominators() {
        // The vacuous 0/0 case can't be reached for the aggregate (eval
        // refuses a corpus with zero RELEVANT labels before any AI call) —
        // 1.0 here only shields per-fixture math from NaN.
        assert_eq!(ratio(0, 0), 1.0);
        assert_eq!(ratio(1, 2), 0.5);
    }

    fn region(path: &str, s: u64, e: u64, importance: &str) -> LabeledRegion {
        LabeledRegion { path: path.into(), start_line: s, end_line: e, importance: importance.into(), note: String::new() }
    }

    #[test]
    fn shipped_snapshots_load_head_base_and_sibling_repos() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../corpus/fixtures");
        let s = load_snapshot(&fixtures.join("cross-repo-contract-rs"), "cross-repo-contract-rs").unwrap();
        let head = &s.files["cross-repo-contract-rs"]["head"];
        assert!(head["src/jobs.rs"].contains("rename_all"));
        assert!(!s.files["cross-repo-contract-rs"]["base"]["src/jobs.rs"].contains("rename_all"));
        assert!(s.files["web"]["default"]["src/jobs/StatusBadge.tsx"].contains("\"InProgress\""));
        // A fixture without repo/ gets an empty snapshot, not an error.
        let empty = load_snapshot(&fixtures.join("planted-bug-rs"), "planted-bug-rs").unwrap();
        assert!(empty.files.is_empty());
    }

    #[test]
    fn verdict_scoring_matches_labels_and_flags_absence() {
        let v = |s: &str| ReviewVerdict { verdict: s.to_string(), reason: String::new() };
        let mut f = FindingsScore::default();
        score_verdict(&mut f, Some(&v("fix_first")), Some("fix_first"));
        assert_eq!((f.verdict.as_deref(), f.verdict_match), (Some("fix_first"), Some(true)));
        let mut f = FindingsScore::default();
        score_verdict(&mut f, None, Some("ship"));
        assert_eq!(f.verdict_match, Some(false));
        assert!(f.detail[0].contains("expected ship, got none"));
        let mut f = FindingsScore::default();
        score_verdict(&mut f, Some(&v("ship")), None);
        assert_eq!(f.verdict_match, None, "unlabeled fixtures don't score the verdict");
    }

    #[test]
    fn completeness_counts_only_substantive_findings() {
        let labels: FixtureLabels = serde_json::from_str(r#"{"relevant":["a.rs"],"not_relevant":[]}"#).unwrap();
        let h = |cat: &str, scenario: &str, fix: &str| HighlightResult {
            path: "a.rs".into(),
            start_line: 1,
            end_line: 1,
            category: cat.into(),
            scenario: scenario.into(),
            fix: fix.into(),
            ..Default::default()
        };
        let s = score_findings(
            &[h("bug", "x", "y"), h("test_gap", "x", ""), h("observation", "", ""), h("behavior", " ", "y")],
            &labels,
        );
        assert_eq!((s.complete, s.substantive), (1, 3));
    }

    #[test]
    fn label_validation_rejects_unknown_verdict() {
        let pr: FixturePr = serde_json::from_str(r#"{"title":"t","body":"","files":[{"path":"a.rs","diff":"+x"}]}"#).unwrap();
        let bad: FixtureLabels =
            serde_json::from_str(r#"{"relevant":["a.rs"],"not_relevant":[],"expected_verdict":"lgtm"}"#).unwrap();
        assert!(validate_labels(&pr, &bad, "f").unwrap_err().contains("expected_verdict"));
    }

    fn highlight(path: &str, s: u64, e: u64) -> HighlightResult {
        HighlightResult { path: path.into(), start_line: s, end_line: e, severity: "warning".into(), comment: "c".into(), ..Default::default() }
    }

    #[test]
    fn findings_scoring_buckets_found_missed_lowvalue_extra() {
        let labels = FixtureLabels {
            relevant: vec![],
            not_relevant: vec![],
            expected_findings: vec![region("a.rs", 20, 30, "important"), region("a.rs", 50, 55, "minor")],
            should_not_flag: vec![region("b.rs", 3, 6, "important")],
            expected_coverage: vec![],
            expected_verdict: None,
            risk_checks: vec![],
        };
        let highlights = vec![
            highlight("a.rs", 25, 27),  // overlaps the important region → found
            highlight("b.rs", 4, 4),    // flags the stated purpose → low-value
            highlight("c.rs", 1, 2),    // matches nothing → extra (neutral)
        ];
        let s = score_findings(&highlights, &labels);
        assert_eq!((s.important_found, s.important_missed), (1, 0));
        assert_eq!((s.minor_found, s.minor_missed), (0, 1), "the minor region went unflagged");
        assert_eq!(s.low_value, 1);
        assert_eq!(s.extra, 1);
    }

    #[test]
    fn overlap_requires_same_path_and_range_intersection() {
        let l = region("a.rs", 10, 20, "important");
        assert!(overlaps(20, 25, &l, "a.rs"), "touching at the boundary counts");
        assert!(overlaps(5, 10, &l, "a.rs"));
        assert!(!overlaps(21, 30, &l, "a.rs"));
        assert!(!overlaps(10, 20, &l, "other.rs"), "same lines, wrong file");
    }

    #[test]
    fn label_relevant_diffs_feeds_only_ground_truth_files() {
        let files = vec![
            FixtureFile { path: "src/a.rs".into(), diff: "A".into() },
            FixtureFile { path: "gen/b.rs".into(), diff: "B".into() },
            FixtureFile { path: "src/c.rs".into(), diff: "C".into() },
        ];
        let relevant = vec!["src/a.rs".to_string(), "src/c.rs".to_string()];
        let diffs = label_relevant_diffs(&files, &relevant);
        assert_eq!(
            diffs,
            vec![("src/a.rs".into(), "A".into()), ("src/c.rs".into(), "C".into())],
            "only label-RELEVANT files, in fixture order"
        );
    }

    #[test]
    fn labels_v2_optional_lists_parse_and_default() {
        // v1-shaped labels.json (no findings lists) must keep parsing.
        let v1: FixtureLabels =
            serde_json::from_str(r#"{ "relevant": ["a.rs"], "not_relevant": [] }"#).unwrap();
        assert!(v1.expected_findings.is_empty() && v1.should_not_flag.is_empty());

        let v2: FixtureLabels = serde_json::from_str(
            r#"{
                "relevant": ["a.rs"], "not_relevant": [],
                "expected_findings": [
                    { "path": "a.rs", "start_line": 20, "end_line": 30, "note": "n" }
                ],
                "should_not_flag": [
                    { "path": "a.rs", "start_line": 1, "end_line": 2, "importance": "minor", "note": "n" }
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(v2.expected_findings[0].importance, "important", "importance defaults");
        assert_eq!(v2.should_not_flag[0].importance, "minor");
    }

    /// The shipped corpus must parse and validate under the current schema.
    /// VERSION must be ≥ 2 (the findings-labels schema) and numeric, but is
    /// not pinned — it bumps with every fixture change by design.
    fn risk(expected: &str) -> LabeledRisk {
        LabeledRisk { title: format!("{expected} risk"), detail: "d".into(), path: "a.rs".into(), start_line: Some(1), expected: expected.into() }
    }

    fn check(outcome: &str) -> Option<RiskCheck> {
        Some(RiskCheck { outcome: outcome.into(), reason: "because".into() })
    }

    #[test]
    fn risk_scoring_separates_false_clears_from_false_confirms() {
        let labels = [risk("confirmed"), risk("confirmed"), risk("cleared"), risk("cleared"), risk("confirmed"), risk("cleared")];
        let checks = [check("confirmed"), check("cleared"), check("confirmed"), check("cleared"), check("unresolved"), None];
        // The first confirmed risk (a.rs:1) has a finding nearby; none for the rest.
        let findings = [HighlightResult { path: "a.rs".into(), start_line: 5, end_line: 8, ..Default::default() }];
        let s = score_risk_checks(&labels, &checks, &findings);
        assert_eq!(
            (s.labeled, s.correct, s.false_clears, s.false_confirms, s.unresolved, s.unanswered),
            (6, 2, 1, 1, 1, 1)
        );
        assert!(s.detail[1].starts_with("RISK ✗ confirmed risk (expected confirmed, got cleared): because"));
        assert_eq!(s.confirmed_unanchored, 0, "a.rs:1 is within the window of L5-8");
        // Fewer answers than risks: the rest are unanswered, not a panic.
        assert_eq!(score_risk_checks(&labels[..2], &[], &[]).unanswered, 2);
        // A confirmed risk with no finding near it is counted.
        let far = [HighlightResult { path: "a.rs".into(), start_line: 50, end_line: 50, ..Default::default() }];
        let s = score_risk_checks(&labels[..1], &[check("confirmed")], &far);
        assert_eq!((s.correct, s.confirmed_unanchored), (1, 1));
    }

    #[test]
    fn label_validation_rejects_bad_risk_labels() {
        let pr: FixturePr = serde_json::from_str(r#"{"title":"t","body":"","files":[{"path":"a.rs","diff":""},{"path":"b.md","diff":""}]}"#).unwrap();
        let bad_outcome: FixtureLabels = serde_json::from_str(
            r#"{"relevant":["a.rs"],"not_relevant":["b.md"],"risk_checks":[{"title":"t","detail":"d","path":"a.rs","expected":"unresolved"}]}"#,
        )
        .unwrap();
        assert!(validate_labels(&pr, &bad_outcome, "f").unwrap_err().contains("expects"));
        let not_relevant: FixtureLabels = serde_json::from_str(
            r#"{"relevant":["a.rs"],"not_relevant":["b.md"],"risk_checks":[{"title":"t","detail":"d","path":"b.md","expected":"cleared"}]}"#,
        )
        .unwrap();
        assert!(validate_labels(&pr, &not_relevant, "f").unwrap_err().contains("not label-relevant"));
    }

    #[test]
    fn the_usage_line_shows_cache_reads_and_the_best_cost_known() {
        let u = AiUsage {
            connection: "anthropic-api".into(),
            calls: 4,
            input_tokens: 1200,
            cache_read_tokens: 90_000,
            cache_write_tokens: 30_000,
            output_tokens: 800,
            list_cost_usd: Some(0.31),
            ..Default::default()
        };
        assert_eq!(
            usage_line(&u),
            "AI USAGE 4 calls via anthropic-api · input 1200 · cache read 90000 · cache write 30000 · output 800 · ≈$0.31 at list price"
        );
        assert!(usage_line(&AiUsage { reported_cost_usd: Some(1.5), ..u.clone() }).ends_with("$1.50 reported"));
        let cut = AiUsage {
            interrupted_calls: 2,
            cut_short_usage: marrow_core::usage::CallUsage { input_tokens: 40, cache_read_tokens: 180_000, ..Default::default() },
            ..u
        };
        assert!(usage_line(&cut).ends_with(
            "· 2 cut-short call(s) not included (before the cut: input 40 · cache read 180000 · cache write 0)"
        ));
    }

    #[test]
    fn a_fixtures_usage_is_the_difference_between_snapshots() {
        let before = AiUsage { calls: 3, input_tokens: 100, cache_read_tokens: 1000, reported_cost_usd: Some(0.5), ..Default::default() };
        let now = AiUsage {
            calls: 7,
            interrupted_calls: 1,
            input_tokens: 150,
            cache_read_tokens: 91_000,
            cache_write_tokens: 20,
            output_tokens: 40,
            reported_cost_usd: Some(0.8),
            cut_short_usage: marrow_core::usage::CallUsage { input_tokens: 9, cache_read_tokens: 45_000, ..Default::default() },
            ..Default::default()
        };
        let d: std::collections::HashMap<_, _> = usage_delta(&now, &before).into_iter().collect();
        assert_eq!(d["calls"], 4);
        assert_eq!(d["interrupted_calls"], 1);
        assert_eq!((d["input_tokens"].clone(), d["cache_read_tokens"].clone(), d["cache_write_tokens"].clone(), d["output_tokens"].clone()),
                   (50.into(), 90_000.into(), 20.into(), 40.into()));
        assert_eq!((d["cut_short_input_tokens"].clone(), d["cut_short_cache_read_tokens"].clone()), (9.into(), 45_000.into()));
        assert!((d["reported_cost_usd"].as_f64().unwrap() - 0.3).abs() < 1e-9);
        // The run's first fixture starts from an empty meter.
        let first: std::collections::HashMap<_, _> =
            usage_delta(&AiUsage { calls: 2, reported_cost_usd: Some(0.2), ..Default::default() }, &AiUsage::default()).into_iter().collect();
        assert_eq!(first["reported_cost_usd"], serde_json::json!(0.2));
        // A cost that stopped being reported mid-run isn't invented.
        let lost: std::collections::HashMap<_, _> =
            usage_delta(&AiUsage { calls: 5, ..Default::default() }, &before).into_iter().collect();
        assert!(lost["reported_cost_usd"].is_null());
    }

    #[test]
    fn shipped_corpus_parses_and_validates() {
        let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../corpus");
        let version = fs::read_to_string(corpus.join("VERSION")).unwrap();
        assert!(version.trim().parse::<u32>().unwrap() >= 2);
        let mut seen = 0;
        for entry in fs::read_dir(corpus.join("fixtures")).unwrap() {
            let dir = entry.unwrap().path();
            if !dir.is_dir() {
                continue;
            }
            let name = dir.file_name().unwrap().to_string_lossy().to_string();
            let pr: FixturePr = read_json(&dir.join("pr.json")).unwrap();
            let labels: FixtureLabels = read_json(&dir.join("labels.json")).unwrap();
            validate_labels(&pr, &labels, &name).unwrap();
            seen += 1;
        }
        assert!(seen >= 6, "corpus unexpectedly small: {seen} fixtures");

        // The findings yardstick itself must stay in place: planted-bug-rs
        // carries the planted important finding and a should_not_flag region.
        let planted: FixtureLabels =
            read_json(&corpus.join("fixtures/planted-bug-rs/labels.json")).unwrap();
        assert!(
            planted
                .expected_findings
                .iter()
                .any(|r| r.path == "src/auth/refresh.rs" && r.importance == "important"),
            "planted-bug-rs lost its planted important finding"
        );
        assert!(
            planted.expected_findings.iter().any(|r| r.importance == "minor"),
            "planted-bug-rs lost its minor naming-nit region"
        );
        assert!(!planted.should_not_flag.is_empty(), "planted-bug-rs lost its should_not_flag region");

        // The risk-check yardstick (issue #243): moved-guard-ts hands the
        // review two look-alike risks, one real and one guarded.
        let guard: FixtureLabels = read_json(&corpus.join("fixtures/moved-guard-ts/labels.json")).unwrap();
        for expected in ["confirmed", "cleared"] {
            assert!(guard.risk_checks.iter().any(|r| r.expected == expected), "moved-guard-ts lost its {expected} risk");
        }

        // The coverage yardstick must stay in place too: coverage-upload-ts
        // labels all three statuses and its hallucination bait must remain
        // baited — mentioned in the body, absent from the diff.
        let covdir = corpus.join("fixtures/coverage-upload-ts");
        let cov_labels: FixtureLabels = read_json(&covdir.join("labels.json")).unwrap();
        for status in ["covered", "partial", "uncovered"] {
            assert!(
                cov_labels.expected_coverage.iter().any(|e| e.status == status),
                "coverage-upload-ts lost its {status} expectation"
            );
        }
        let cov_pr: FixturePr = read_json(&covdir.join("pr.json")).unwrap();
        assert!(cov_pr.body.contains("tests/upload.e2e.ts"), "hallucination bait gone from the body");
        assert!(
            !cov_pr.files.iter().any(|f| f.path == "tests/upload.e2e.ts"),
            "the bait path must NOT exist in the diff or it stops being bait"
        );
    }

    #[test]
    fn retry_pass_recovers_from_transient_failures() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        // Fails twice (truncated garbage), succeeds on the final attempt.
        let mut calls = 0;
        let out: Result<Vec<serde_json::Value>, String> = rt.block_on(retry_json_pass("findings", "f", || {
            calls += 1;
            let resp = if calls < PASS_ATTEMPTS { "[{\"trunca".to_string() } else { "[]".to_string() };
            async move { Ok(resp) }
        }));
        assert_eq!(calls, PASS_ATTEMPTS);
        assert!(out.unwrap().is_empty());

        // Exhausted retries: the error names the pass and attempt count and
        // carries the last underlying failure.
        let mut calls = 0;
        let out: Result<Vec<serde_json::Value>, String> = rt.block_on(retry_json_pass("classification", "f", || {
            calls += 1;
            async { Err("provider exploded".to_string()) }
        }));
        assert_eq!(calls, PASS_ATTEMPTS);
        let e = out.unwrap_err();
        assert!(e.contains("classification failed after 3 attempts"), "{e}");
        assert!(e.contains("provider exploded"), "{e}");
    }

    fn req(text: &str, status: &str, tests: &[&str]) -> marrow_core::types::RequirementEntry {
        marrow_core::types::RequirementEntry {
            text: text.into(),
            status: status.into(),
            tests: tests.iter().map(|p| marrow_core::types::TestRef { path: (*p).into(), note: None }).collect(),
            note: None,
        }
    }

    fn exp(contains: &str, status: &str) -> ExpectedCoverage {
        ExpectedCoverage { requirement_contains: contains.into(), status: status.into() }
    }

    #[test]
    fn coverage_inputs_split_by_the_cores_own_detectors() {
        let files = vec![
            FixtureFile { path: "tests/upload.test.ts".into(), diff: "+test()".into() },
            FixtureFile { path: "src/plain.rs".into(), diff: "+fn f() {}".into() },
            FixtureFile { path: "src/inline.rs".into(), diff: "+#[cfg(test)]\n+mod tests {\n+    #[test]\n+    fn t() {}\n+}".into() },
        ];
        let (test_diffs, inline) = split_coverage_inputs(&files);
        assert_eq!(test_diffs.len(), 1, "only the path-convention test file");
        assert_eq!(test_diffs[0].0, "tests/upload.test.ts");
        assert_eq!(inline.len(), 1, "only the impl diff that adds inline tests");
        assert_eq!(inline[0].0, "src/inline.rs");
    }

    #[test]
    fn coverage_scoring_buckets_match_mismatch_missing_extra() {
        let cov = RequirementsCoverage {
            requirements: vec![
                req("Retries a failed upload up to 3 times", "covered", &["tests/upload.test.ts"]),
                req("Shows a toast on permanent failure", "uncovered", &[]),
                req("Bonus requirement nobody labeled", "covered", &[]),
            ],
            orphan_tests: vec![],
            source_issues: vec![],
        };
        let expected = vec![
            exp("UP TO 3 TIMES", "covered"),      // match — needle case must not matter
            exp("toast", "partial"),               // mismatch: got uncovered
            exp("parallel batches", "uncovered"),  // never extracted
        ];
        let s = score_coverage(Some(&cov), &expected);
        assert_eq!((s.status_match, s.status_mismatch, s.not_extracted, s.extra), (1, 1, 1, 1));
        assert!(s.detail.iter().any(|d| d.contains("expected partial, got uncovered")), "{:?}", s.detail);
        assert!(s.detail.iter().any(|d| d.contains("no requirement extracted")), "{:?}", s.detail);

        // Finalize dropped everything → every expectation is not-extracted.
        let s = score_coverage(None, &expected);
        assert_eq!((s.status_match, s.not_extracted), (0, 3));
    }

    #[test]
    fn hallucinated_citations_counted_on_raw_parse() {
        let cov = RequirementsCoverage {
            requirements: vec![req("r1", "covered", &["tests/real.test.ts", "tests/upload.e2e.ts"])],
            orphan_tests: vec![marrow_core::types::TestRef { path: "tests/ghost.test.ts".into(), note: None }],
            source_issues: vec![],
        };
        let known: HashSet<&str> = ["tests/real.test.ts"].into_iter().collect();
        let (n, detail) = count_hallucinated_citations(&cov, &known);
        assert_eq!(n, 2);
        assert!(detail.iter().any(|d| d.contains("upload.e2e.ts")));
        assert!(detail.iter().any(|d| d.contains("ghost.test.ts")));
    }

    #[test]
    fn label_validation_rejects_bad_coverage_expectations() {
        let pr = FixturePr {
            title: "t".into(),
            body: "b".into(),
            files: vec![FixtureFile { path: "a.rs".into(), diff: String::new() }],
        };
        let bad_status = FixtureLabels {
            relevant: vec!["a.rs".into()],
            not_relevant: vec![],
            expected_findings: vec![],
            should_not_flag: vec![],
            expected_coverage: vec![exp("retries", "mostly-covered")],
            expected_verdict: None,
            risk_checks: vec![],
        };
        assert!(validate_labels(&pr, &bad_status, "f").is_err());
        let empty_needle = FixtureLabels {
            relevant: vec!["a.rs".into()],
            not_relevant: vec![],
            expected_findings: vec![],
            should_not_flag: vec![],
            expected_coverage: vec![exp("  ", "covered")],
            expected_verdict: None,
            risk_checks: vec![],
        };
        assert!(validate_labels(&pr, &empty_needle, "f").is_err());
        let valid = FixtureLabels {
            relevant: vec!["a.rs".into()],
            not_relevant: vec![],
            expected_findings: vec![],
            should_not_flag: vec![],
            expected_coverage: vec![exp("retries", "covered"), exp("toast", "untestable")],
            expected_verdict: None,
            risk_checks: vec![],
        };
        assert!(validate_labels(&pr, &valid, "f").is_ok());
    }

    #[test]
    fn report_names_failed_passes_without_overstating() {
        let clean = FixtureScore { name: "ok-fixture".into(), true_pos: 2, false_pos: 0, false_neg: 0, mismatches: vec![], findings: None, coverage: None, failed: None, failed_pass: None };
        let findings_failed = FixtureScore {
            name: "flaky-findings".into(),
            true_pos: 1, false_pos: 0, false_neg: 0,
            mismatches: vec![],
            findings: None,
            coverage: None,
            failed: Some("findings failed after 3 attempts: truncated".into()),
            failed_pass: Some("findings"),
        };
        let class_failed = FixtureScore {
            name: "dead-fixture".into(),
            true_pos: 0, false_pos: 0, false_neg: 0,
            mismatches: vec![],
            findings: None,
            coverage: None,
            failed: Some("classification failed after 3 attempts: boom".into()),
            failed_pass: Some("classification"),
        };
        let report = render_text_report(&[clean, findings_failed, class_failed], "3", 1.0, 1.0);
        // A findings-only failure keeps the (valid) classification verdict.
        assert!(report.contains("clean · findings FAILED"), "{report}");
        assert!(!report.contains("flaky-findings           tp=1 fp=0 fn=0  FAILED\n"), "findings failure must not read as a whole-fixture FAILED:\n{report}");
        assert!(report.contains("FAILED (classification)"), "{report}");
        assert!(report.contains("⚠ 2 of 3 fixture(s) had a failed pass"), "{report}");
        // A coverage-pass failure gets the same pass-specific treatment.
        let coverage_failed = FixtureScore {
            name: "flaky-coverage".into(),
            true_pos: 1, false_pos: 0, false_neg: 0,
            mismatches: vec![],
            findings: None,
            coverage: None,
            failed: Some("coverage failed after 3 attempts: truncated".into()),
            failed_pass: Some("coverage"),
        };
        let r2 = render_text_report(&[coverage_failed], "4", 1.0, 1.0);
        assert!(r2.contains("clean · coverage FAILED"), "{r2}");
        assert!(report.contains("numbers below cover completed passes only"), "{report}");
        // No failures → no warning line.
        let ok = FixtureScore { name: "ok".into(), true_pos: 1, false_pos: 0, false_neg: 0, mismatches: vec![], findings: None, coverage: None, failed: None, failed_pass: None };
        assert!(!render_text_report(&[ok], "3", 1.0, 1.0).contains('⚠'));
    }

    #[test]
    fn failed_passes_make_the_run_exit_nonzero_after_reporting() {
        let ok = FixtureScore { name: "ok".into(), true_pos: 1, false_pos: 0, false_neg: 0, mismatches: vec![], findings: None, coverage: None, failed: None, failed_pass: None };
        assert!(completion_status(&[ok]).is_ok());
        let failed = FixtureScore {
            name: "flaky".into(),
            true_pos: 0, false_pos: 0, false_neg: 0,
            mismatches: vec![],
            findings: None,
            coverage: None,
            failed: Some("findings failed after 3 attempts: truncated".into()),
            failed_pass: Some("findings"),
        };
        let ok2 = FixtureScore { name: "ok".into(), true_pos: 1, false_pos: 0, false_neg: 0, mismatches: vec![], findings: None, coverage: None, failed: None, failed_pass: None };
        let e = completion_status(&[failed, ok2]).unwrap_err();
        assert!(e.contains("1 of 2 fixture(s)"), "{e}");
    }

    #[test]
    fn json_report_carries_failed_outcome() {
        let findings_failed = FixtureScore {
            name: "flaky".into(),
            true_pos: 1, false_pos: 0, false_neg: 0,
            mismatches: vec![],
            findings: None,
            coverage: None,
            failed: Some("findings failed after 3 attempts: truncated".into()),
            failed_pass: Some("findings"),
        };
        let ok = FixtureScore { name: "ok".into(), true_pos: 1, false_pos: 0, false_neg: 0, mismatches: vec![], findings: None, coverage: None, failed: None, failed_pass: None };
        let out = render_json_report(&[findings_failed, ok], "3", "m", 1.0, 1.0);
        let fx = out["fixtures"].as_array().unwrap();
        assert_eq!(fx[0]["failed"], "findings failed after 3 attempts: truncated");
        assert_eq!(fx[0]["failed_pass"], "findings");
        assert!(fx[0]["findings"].is_null());
        assert!(fx[1]["failed"].is_null(), "clean fixtures report null, not absent-by-accident");
        assert_eq!(out["corpus_version"], "3");
    }

    /// A broken corpus must fail fast — before load_settings/AiBackend, so
    /// this test needs no AI configuration at all (issue #226 criterion 3).
    #[test]
    fn eval_fails_fast_on_invalid_labels_before_any_ai() {
        let dir = std::env::temp_dir().join(format!("marrow-eval-failfast-{}", std::process::id()));
        let fixture = dir.join("fixtures/broken");
        fs::create_dir_all(&fixture).unwrap();
        fs::write(dir.join("VERSION"), "3\n").unwrap();
        fs::write(fixture.join("pr.json"), r#"{ "title": "t", "body": "b", "files": [{ "path": "a.rs", "diff": "" }] }"#).unwrap();
        // a.rs labeled in BOTH lists → validate_labels must reject.
        fs::write(fixture.join("labels.json"), r#"{ "relevant": ["a.rs"], "not_relevant": ["a.rs"] }"#).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let err = rt.block_on(eval(&dir, false, true, false, None)).unwrap_err();
        assert!(err.contains("broken"), "error should name the fixture: {err}");
        assert!(err.contains("exactly one"), "{err}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn assembled_diff_never_abuts_headers() {
        let files = vec![
            FixtureFile { path: "a.rs".into(), diff: "@@ -1 +1 @@\n-x\n+y".into() }, // no trailing \n
            FixtureFile { path: "b.rs".into(), diff: "@@ -1 +1 @@\n+z\n".into() },
        ];
        let full = assemble_full_diff(&files);
        assert!(full.contains("+y\ndiff --git a/b.rs"), "separator restored:\n{full}");
        assert!(!full.contains("+ydiff --git"), "headers must never abut");
    }
}
