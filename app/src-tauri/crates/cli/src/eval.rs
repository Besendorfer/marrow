//! `marrow eval` — run the real classification pass over the versioned
//! quality corpus and score precision/recall for RELEVANT (issue #219,
//! roadmap Phase 3). Provider- and model-dependent BY DESIGN: run it before
//! and after a prompt/model change and compare against the same corpus
//! version.

use marrow_core::ai::{extract_json_array, AiBackend};
use marrow_core::config::load_settings;
use marrow_core::fetch::{validate_classifications, validate_highlights};
use marrow_core::prompts::{build_classification_prompt, build_highlight_prompt};
use marrow_core::types::{FileClassification, HighlightResult};
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

        let (prompt, _truncated) = build_classification_prompt(&pr.title, &file_list, &full_diff);
        eprintln!("· {name}: classifying {} files…", file_list.len());
        let raw = ai.invoke(&prompt).await?;
        let parsed: Vec<FileClassification> = serde_json::from_value(extract_json_array(&raw)?)
            .map_err(|e| format!("{name}: unparseable classification: {e}"))?;
        // Judge POST-validation output — the same pipeline the app runs.
        let validated = validate_classifications(parsed, &file_list);

        let predicted_relevant: std::collections::HashSet<&str> = validated
            .iter()
            .filter(|c| c.classification == "RELEVANT")
            .map(|c| c.path.as_str())
            .collect();
        let mut score = FixtureScore { name, true_pos: 0, false_pos: 0, false_neg: 0, mismatches: Vec::new(), findings: None };
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
            let raw = ai.invoke(&hl_prompt).await?;
            let parsed: Vec<HighlightResult> = serde_json::from_value(extract_json_array(&raw)?)
                .map_err(|e| format!("{}: unparseable highlights: {e}", score.name))?;
            let validated = validate_highlights(parsed, &file_list);
            score.findings = Some(score_findings(&validated, &labels));
        }
        scores.push(score);
    }

    let (tp, fp, fneg) = scores.iter().fold((0, 0, 0), |(a, b, c), s| {
        (a + s.true_pos, b + s.false_pos, c + s.false_neg)
    });
    let precision = ratio(tp, tp + fp);
    let recall = ratio(tp, tp + fneg);

    if json {
        let out = serde_json::json!({
            "corpus_version": version,
            "model": settings.model,
            "fixtures": scores.iter().map(|s| serde_json::json!({
                "name": s.name,
                "true_pos": s.true_pos, "false_pos": s.false_pos, "false_neg": s.false_neg,
                "mismatches": s.mismatches,
                "findings": s.findings.as_ref().map(|f| serde_json::json!({
                    "important_found": f.important_found, "important_missed": f.important_missed,
                    "minor_found": f.minor_found, "minor_missed": f.minor_missed,
                    "low_value": f.low_value, "extra": f.extra, "detail": f.detail,
                })),
            })).collect::<Vec<_>>(),
            "precision": precision,
            "recall": recall,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!();
        for s in &scores {
            let verdict = if s.mismatches.is_empty() { "clean" } else { "MISMATCHES" };
            println!("{:<24} tp={} fp={} fn={}  {}", s.name, s.true_pos, s.false_pos, s.false_neg, verdict);
            for m in &s.mismatches {
                println!("    {m}");
            }
            if let Some(f) = &s.findings {
                println!(
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
                    println!("    {d}");
                }
            }
        }
        println!();
        println!("RELEVANT precision {:.2} · recall {:.2} (corpus v{version})", precision, recall);
    }
    Ok(())
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
        let ok = FixtureLabels { relevant: vec!["a.rs".into()], not_relevant: vec!["b.rs".into()], expected_findings: vec![], should_not_flag: vec![] };
        assert!(validate_labels(&pr, &ok, "f").is_ok());
        let overlap = FixtureLabels { relevant: vec!["a.rs".into(), "b.rs".into()], not_relevant: vec!["b.rs".into()], expected_findings: vec![], should_not_flag: vec![] };
        assert!(validate_labels(&pr, &overlap, "f").is_err());
        let missing = FixtureLabels { relevant: vec!["a.rs".into()], not_relevant: vec![], expected_findings: vec![], should_not_flag: vec![] };
        assert!(validate_labels(&pr, &missing, "f").is_err());
        // A typo'd importance must not silently bucket as "important".
        let typo = FixtureLabels {
            relevant: vec!["a.rs".into()],
            not_relevant: vec!["b.rs".into()],
            expected_findings: vec![region("a.rs", 1, 2, "importnat")],
            should_not_flag: vec![],
        };
        assert!(validate_labels(&pr, &typo, "f").is_err());
        // A findings region on a non-relevant path could never be scored.
        let unwinnable = FixtureLabels {
            relevant: vec!["a.rs".into()],
            not_relevant: vec!["b.rs".into()],
            expected_findings: vec![region("b.rs", 1, 2, "important")],
            should_not_flag: vec![],
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
