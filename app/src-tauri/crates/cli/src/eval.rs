//! `marrow eval` — run the real classification pass over the versioned
//! quality corpus and score precision/recall for RELEVANT (issue #219,
//! roadmap Phase 3). Provider- and model-dependent BY DESIGN: run it before
//! and after a prompt/model change and compare against the same corpus
//! version.

use marrow_core::ai::{extract_json_array, AiBackend};
use marrow_core::config::load_settings;
use marrow_core::fetch::validate_classifications;
use marrow_core::prompts::build_classification_prompt;
use marrow_core::types::FileClassification;
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
}

struct FixtureScore {
    name: String,
    true_pos: usize,
    false_pos: usize,
    false_neg: usize,
    mismatches: Vec<String>,
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
        let mut score = FixtureScore { name, true_pos: 0, false_pos: 0, false_neg: 0, mismatches: Vec::new() };
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
        }
        println!();
        println!("RELEVANT precision {:.2} · recall {:.2} (corpus v{version})", precision, recall);
    }
    Ok(())
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
        let ok = FixtureLabels { relevant: vec!["a.rs".into()], not_relevant: vec!["b.rs".into()] };
        assert!(validate_labels(&pr, &ok, "f").is_ok());
        let overlap = FixtureLabels { relevant: vec!["a.rs".into(), "b.rs".into()], not_relevant: vec!["b.rs".into()] };
        assert!(validate_labels(&pr, &overlap, "f").is_err());
        let missing = FixtureLabels { relevant: vec!["a.rs".into()], not_relevant: vec![] };
        assert!(validate_labels(&pr, &missing, "f").is_err());
    }

    #[test]
    fn ratios_handle_empty_denominators() {
        // The vacuous 0/0 case can't be reached for the aggregate (eval
        // refuses a corpus with zero RELEVANT labels before any AI call) —
        // 1.0 here only shields per-fixture math from NaN.
        assert_eq!(ratio(0, 0), 1.0);
        assert_eq!(ratio(1, 2), 0.5);
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
