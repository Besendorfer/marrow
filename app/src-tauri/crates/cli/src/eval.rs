//! `marrow eval` — run the real classification pass over the versioned
//! quality corpus and score precision/recall for RELEVANT (issue #219,
//! roadmap Phase 3). Provider- and model-dependent BY DESIGN: run it before
//! and after a prompt/model change and compare against the same corpus
//! version.

use marrow_core::ai::{extract_json_array, extract_json_object, AiBackend};
use marrow_core::config::load_settings;
use marrow_core::fetch::{finalize_coverage, validate_classifications, validate_highlights};
use marrow_core::prompts::{
    build_classification_prompt, build_highlight_prompt, build_requirements_coverage_prompt,
    has_inline_test_markers, is_test_path,
};
use marrow_core::types::{FileClassification, HighlightResult, RequirementsCoverage};
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
}

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

pub async fn eval(corpus: &Path, json: bool) -> Result<(), String> {
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
    let mut fixtures: Vec<(String, FixturePr, FixtureLabels)> = Vec::new();
    for dir in &fixture_dirs {
        let name = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let pr: FixturePr = read_json(&dir.join("pr.json"))?;
        let labels: FixtureLabels = read_json(&dir.join("labels.json"))?;
        validate_labels(&pr, &labels, &name)?;
        fixtures.push((name, pr, labels));
    }
    // Measurement honesty: with zero RELEVANT labels there is nothing to
    // measure — 1.00/1.00 on an empty corpus would be vacuous, not perfect.
    if fixtures.iter().all(|(_, _, l)| l.relevant.is_empty()) {
        return Err("corpus has no RELEVANT labels — nothing to measure".to_string());
    }

    let settings = load_settings();
    let ai = AiBackend::from_settings(&settings).await?;
    eprintln!(
        "corpus v{version} · {} fixture(s) · model {}",
        fixtures.len(),
        if settings.model.is_empty() { "(claude CLI default)" } else { &settings.model }
    );

    let mut scores: Vec<FixtureScore> = Vec::new();
    for (name, pr, labels) in fixtures {
        let file_list: Vec<String> = pr.files.iter().map(|f| f.path.clone()).collect();
        let full_diff = assemble_full_diff(&pr.files);

        let mut score = FixtureScore { name, true_pos: 0, false_pos: 0, false_neg: 0, mismatches: Vec::new(), findings: None, coverage: None, failed: None, failed_pass: None };

        let (prompt, _truncated) = build_classification_prompt(&pr.title, &file_list, &full_diff);
        eprintln!("· {}: classifying {} files…", score.name, file_list.len());
        let parsed: Vec<FileClassification> =
            match retry_json_pass("classification", &score.name, || ai.invoke(&prompt)).await {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("· {}: {e}", score.name);
                    score.failed = Some(e);
                    score.failed_pass = Some("classification");
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
        if !labels.expected_findings.is_empty() || !labels.should_not_flag.is_empty() {
            let relevant_diffs = label_relevant_diffs(&pr.files, &labels.relevant);
            let (hl_prompt, _t) = build_highlight_prompt(&pr.title, &pr.body, &relevant_diffs, &[]);
            eprintln!("· {}: reviewing for findings…", score.name);
            match retry_json_pass::<HighlightResult, _, _>("findings", &score.name, || ai.invoke(&hl_prompt)).await {
                Ok(parsed) => {
                    let validated = validate_highlights(parsed, &file_list);
                    score.findings = Some(score_findings(&validated, &labels));
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
            let test_diffs: Vec<(String, String)> = pr
                .files
                .iter()
                .filter(|f| is_test_path(&f.path))
                .map(|f| (f.path.clone(), f.diff.clone()))
                .collect();
            let inline_test_diffs: Vec<(String, String)> = pr
                .files
                .iter()
                .filter(|f| !is_test_path(&f.path) && has_inline_test_markers(&f.diff))
                .map(|f| (f.path.clone(), f.diff.clone()))
                .collect();
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
        scores.push(score);
    }

    let (tp, fp, fneg) = scores.iter().fold((0, 0, 0), |(a, b, c), s| {
        (a + s.true_pos, b + s.false_pos, c + s.false_neg)
    });
    let precision = ratio(tp, tp + fp);
    let recall = ratio(tp, tp + fneg);

    if json {
        let out = render_json_report(&scores, &version, &settings.model, precision, recall);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        print!("{}", render_text_report(&scores, &version, precision, recall));
    }
    completion_status(&scores)
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
                "low_value": f.low_value, "extra": f.extra, "detail": f.detail,
            })),
            "coverage": s.coverage.as_ref().map(|c| serde_json::json!({
                "status_match": c.status_match, "status_mismatch": c.status_mismatch,
                "not_extracted": c.not_extracted, "extra": c.extra,
                "hallucinated": c.hallucinated, "detail": c.detail,
            })),
        })).collect::<Vec<_>>(),
        "precision": precision,
        "recall": recall,
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
    out
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
    detail: Vec<String>,
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
    }
    score
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
        let ok = FixtureLabels { relevant: vec!["a.rs".into()], not_relevant: vec!["b.rs".into()], expected_findings: vec![], should_not_flag: vec![], expected_coverage: vec![] };
        assert!(validate_labels(&pr, &ok, "f").is_ok());
        let overlap = FixtureLabels { relevant: vec!["a.rs".into(), "b.rs".into()], not_relevant: vec!["b.rs".into()], expected_findings: vec![], should_not_flag: vec![], expected_coverage: vec![] };
        assert!(validate_labels(&pr, &overlap, "f").is_err());
        let missing = FixtureLabels { relevant: vec!["a.rs".into()], not_relevant: vec![], expected_findings: vec![], should_not_flag: vec![], expected_coverage: vec![] };
        assert!(validate_labels(&pr, &missing, "f").is_err());
        // A typo'd importance must not silently bucket as "important".
        let typo = FixtureLabels {
            relevant: vec!["a.rs".into()],
            not_relevant: vec!["b.rs".into()],
            expected_findings: vec![region("a.rs", 1, 2, "importnat")],
            should_not_flag: vec![],
            expected_coverage: vec![],
        };
        assert!(validate_labels(&pr, &typo, "f").is_err());
        // A findings region on a non-relevant path could never be scored.
        let unwinnable = FixtureLabels {
            relevant: vec!["a.rs".into()],
            not_relevant: vec!["b.rs".into()],
            expected_findings: vec![region("b.rs", 1, 2, "important")],
            should_not_flag: vec![],
            expected_coverage: vec![],
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

    fn highlight(path: &str, s: u64, e: u64) -> HighlightResult {
        HighlightResult { path: path.into(), start_line: s, end_line: e, severity: "warning".into(), comment: "c".into() }
    }

    #[test]
    fn findings_scoring_buckets_found_missed_lowvalue_extra() {
        let labels = FixtureLabels {
            relevant: vec![],
            not_relevant: vec![],
            expected_findings: vec![region("a.rs", 20, 30, "important"), region("a.rs", 50, 55, "minor")],
            should_not_flag: vec![region("b.rs", 3, 6, "important")],
            expected_coverage: vec![],
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
            exp("up to 3 times", "covered"),      // match
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
        };
        assert!(validate_labels(&pr, &bad_status, "f").is_err());
        let empty_needle = FixtureLabels {
            relevant: vec!["a.rs".into()],
            not_relevant: vec![],
            expected_findings: vec![],
            should_not_flag: vec![],
            expected_coverage: vec![exp("  ", "covered")],
        };
        assert!(validate_labels(&pr, &empty_needle, "f").is_err());
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
        let err = rt.block_on(eval(&dir, false)).unwrap_err();
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
