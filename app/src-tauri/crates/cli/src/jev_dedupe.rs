//! `marrow eval --jev-dedupe` (issue #249, third use): can Jev tell when two
//! findings are one problem said twice (merge), one root cause with different
//! actions (group), or separate problems (keep apart)? Scored against the
//! hand-labeled pairs in `corpus/jev-dedupe.json`. No review calls.

use marrow_core::config::{load_settings, resolve_jev_api_key};
use marrow_core::jev::{judge_pair, PairJudgement};
use marrow_core::types::HighlightResult;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const RELATIONS: [&str; 3] = ["same", "related", "different"];

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Side {
    pub path: String,
    pub start_line: u64,
    pub end_line: u64,
    pub category: String,
    pub claim: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Pair {
    pub fixture: String,
    /// "same" | "related" | "different"
    pub label: String,
    pub a: Side,
    pub b: Side,
}

#[derive(Deserialize)]
struct PairFile {
    pairs: Vec<Pair>,
}

#[derive(Deserialize)]
struct FixturePr {
    title: String,
    body: String,
    files: Vec<FixtureFile>,
}

#[derive(Deserialize)]
struct FixtureFile {
    path: String,
    diff: String,
}

/// Jev's relation name → the label vocabulary.
pub fn relation_label(relation: &str) -> &'static str {
    match relation {
        "same_issue" => "same",
        "related" => "related",
        _ => "different",
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    /// confusion[label][predicted] counts.
    pub confusion: BTreeMap<String, BTreeMap<String, usize>>,
    pub accuracy: f64,
    /// Of pairs Jev would merge (predicted same), how many really are one problem.
    pub merge_precision: f64,
    /// Different problems Jev would merge — the error that hides a finding.
    pub different_merged: usize,
    /// P(same) ranks true duplicates over everything else.
    pub auc_same: f64,
    pub errors: Vec<String>,
}

pub fn score(pairs: &[Pair], results: &[(usize, PairJudgement)], errors: Vec<String>) -> Report {
    let mut confusion: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let (mut right, mut merged, mut merged_right, mut different_merged) = (0, 0, 0, 0);
    for (i, j) in results {
        let label = pairs[*i].label.as_str();
        let pred = relation_label(&j.relation);
        *confusion.entry(label.to_string()).or_default().entry(pred.to_string()).or_default() += 1;
        if label == pred {
            right += 1;
        }
        if pred == "same" {
            merged += 1;
            if label == "same" {
                merged_right += 1;
            }
            if label == "different" {
                different_merged += 1;
            }
        }
    }
    let same: Vec<f64> = results.iter().filter(|(i, _)| pairs[*i].label == "same").map(|(_, j)| j.p_same).collect();
    let rest: Vec<f64> = results.iter().filter(|(i, _)| pairs[*i].label != "same").map(|(_, j)| j.p_same).collect();
    let r = |a: usize, b: usize| if b == 0 { 0.0 } else { a as f64 / b as f64 };
    Report {
        confusion,
        accuracy: r(right, results.len()),
        merge_precision: r(merged_right, merged),
        different_merged,
        auc_same: crate::jev_probe::auc(&same, &rest),
        errors,
    }
}

fn highlight(s: &Side) -> HighlightResult {
    HighlightResult {
        path: s.path.clone(),
        start_line: s.start_line,
        end_line: s.end_line,
        category: s.category.clone(),
        comment: s.claim.clone(),
        ..Default::default()
    }
}

pub async fn run(corpus: &Path, json: bool) -> Result<(), String> {
    let key = resolve_jev_api_key(&load_settings())
        .ok_or("--jev-dedupe needs a TypeSafe API key: set it in Settings or TYPESAFE_API_KEY")?;
    let file: PairFile = serde_json::from_str(
        &std::fs::read_to_string(corpus.join("jev-dedupe.json")).map_err(|e| format!("Failed to read jev-dedupe.json: {e}"))?,
    )
    .map_err(|e| format!("jev-dedupe.json: {e}"))?;
    let pairs = file.pairs;
    let mut prs: BTreeMap<String, FixturePr> = BTreeMap::new();
    for p in &pairs {
        if !RELATIONS.contains(&p.label.as_str()) {
            return Err(format!("pair on {}: label must be same, related, or different", p.fixture));
        }
        if !prs.contains_key(&p.fixture) {
            let raw = std::fs::read_to_string(corpus.join("fixtures").join(&p.fixture).join("pr.json"))
                .map_err(|e| format!("pair fixture {}: {e}", p.fixture))?;
            prs.insert(p.fixture.clone(), serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", p.fixture))?);
        }
        for side in [&p.a, &p.b] {
            if !prs[&p.fixture].files.iter().any(|f| f.path == side.path) {
                return Err(format!("pair on {} names {}, which isn't in the fixture", p.fixture, side.path));
            }
        }
    }
    eprintln!("{} pair(s) · TypeSafe jev-latest", pairs.len());
    let (mut results, mut errors) = (Vec::new(), Vec::new());
    for (i, p) in pairs.iter().enumerate() {
        let pr = &prs[&p.fixture];
        let mut paths = vec![p.a.path.clone()];
        if p.b.path != p.a.path {
            paths.push(p.b.path.clone());
        }
        let diffs: Vec<(String, String)> = paths
            .iter()
            .map(|path| (path.clone(), pr.files.iter().find(|f| &f.path == path).map(|f| f.diff.clone()).unwrap_or_default()))
            .collect();
        match judge_pair(&key, &pr.title, &pr.body, &highlight(&p.a), &highlight(&p.b), &diffs).await {
            Ok(j) => results.push((i, j)),
            Err(e) => errors.push(format!("{} pair {i}: {e}", p.fixture)),
        }
        eprint!(".");
        tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
    }
    eprintln!();
    let report = score(&pairs, &results, errors);
    if json {
        let rows: Vec<_> = results.iter().map(|(i, j)| serde_json::json!({ "pair": pairs[*i], "judgement": j })).collect();
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "report": report, "rows": rows })).unwrap());
    } else {
        print!("{}", render_text(&pairs, &results, &report));
    }
    Ok(())
}

pub fn render_text(pairs: &[Pair], results: &[(usize, PairJudgement)], r: &Report) -> String {
    use std::fmt::Write;
    let mut out = format!(
        "\nJEV DEDUPE: {} pairs · accuracy {:.2} · merge precision {:.2} · different-merged {} · AUC(same) {:.2}\n",
        results.len(),
        r.accuracy,
        r.merge_precision,
        r.different_merged,
        r.auc_same
    );
    let _ = writeln!(out, "{:<10} {:>6} {:>8} {:>10}   (rows = label, columns = Jev)", "", "same", "related", "different");
    for label in RELATIONS {
        let row = r.confusion.get(label);
        let c = |p: &str| row.and_then(|m| m.get(p)).copied().unwrap_or(0);
        let _ = writeln!(out, "{:<10} {:>6} {:>8} {:>10}", label, c("same"), c("related"), c("different"));
    }
    for (i, j) in results {
        let p = &pairs[*i];
        let mark = if relation_label(&j.relation) == p.label { " " } else { "✗" };
        let _ = writeln!(
            out,
            "  {mark} {:<9} → {:<10} same {:.2} related {:.2} diff {:.2}  {} {}/{} vs {}/{}",
            p.label, j.relation, j.p_same, j.p_related, j.p_different, p.fixture, p.a.category, p.a.start_line, p.b.category, p.b.start_line
        );
    }
    for e in &r.errors {
        let _ = writeln!(out, "  ERROR {e}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(label: &str) -> Pair {
        let side = Side { path: "a.rs".into(), start_line: 1, end_line: 1, category: "bug".into(), claim: "c".into() };
        Pair { fixture: "fx".into(), label: label.into(), a: side.clone(), b: side }
    }

    fn judged(relation: &str, p_same: f64) -> PairJudgement {
        PairJudgement { relation: relation.into(), p_same, p_related: 0.0, p_different: 1.0 - p_same, confidence: 0.5 }
    }

    #[test]
    fn merging_different_problems_is_counted_as_the_costly_error() {
        let pairs = vec![pair("same"), pair("different"), pair("related")];
        let r = score(&pairs, &[(0, judged("same_issue", 0.9)), (1, judged("same_issue", 0.8)), (2, judged("related", 0.2))], vec![]);
        assert_eq!(r.different_merged, 1);
        assert_eq!(r.merge_precision, 0.5);
        assert!((r.accuracy - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(r.confusion["different"]["same"], 1);
    }

    #[test]
    fn the_corpus_pair_file_parses_and_names_real_fixture_files() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../corpus");
        let file: PairFile = serde_json::from_str(&std::fs::read_to_string(dir.join("jev-dedupe.json")).unwrap()).unwrap();
        for p in &file.pairs {
            assert!(RELATIONS.contains(&p.label.as_str()));
            let pr: FixturePr = serde_json::from_str(&std::fs::read_to_string(dir.join("fixtures").join(&p.fixture).join("pr.json")).unwrap()).unwrap();
            for side in [&p.a, &p.b] {
                assert!(pr.files.iter().any(|f| f.path == side.path), "{}:{}", p.fixture, side.path);
            }
        }
    }
}
