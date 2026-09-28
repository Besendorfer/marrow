//! Jev as the file-relevance classifier (issue #249, second use): measure it
//! against the corpus labels next to the LLM pass (`eval --jev-classify`),
//! and against the LLM's cached calls on real PRs (`jev-agree`). Pure scoring
//! and rendering here; the runners do the calls.

use marrow_core::config::{load_settings, resolve_jev_api_key};
use marrow_core::jev::classify_file;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, Instant};

/// Spacing between Jev calls (~30/minute).
pub const JEV_SPACING: Duration = Duration::from_millis(2_100);

/// One file, classified both ways. `label` is ground truth in the corpus run
/// and the LLM's cached call in the manifest run.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub source: String,
    pub path: String,
    pub label: bool,
    pub llm: Option<bool>,
    pub p_jev: Option<f64>,
    pub llm_risk: Option<String>,
    pub jev_risk: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct PR {
    pub precision: f64,
    pub recall: f64,
    pub accuracy: f64,
    pub n: usize,
}

fn pr_of(pairs: impl Iterator<Item = (bool, bool)>) -> PR {
    let (mut tp, mut fp, mut fneg, mut right, mut n) = (0, 0, 0, 0, 0);
    for (label, pred) in pairs {
        n += 1;
        match (label, pred) {
            (true, true) => { tp += 1; right += 1; }
            (false, true) => fp += 1,
            (true, false) => fneg += 1,
            (false, false) => right += 1,
        }
    }
    let r = |a: usize, b: usize| if b == 0 { 1.0 } else { a as f64 / b as f64 };
    PR { precision: r(tp, tp + fp), recall: r(tp, tp + fneg), accuracy: r(right, n), n }
}

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub llm: Option<PR>,
    pub jev: PR,
    /// P(relevant) ranks labeled-relevant files over the rest.
    pub jev_auc: f64,
    pub risk_agreement: Option<f64>,
    pub disagreements: Vec<String>,
    pub errors: usize,
    pub jev_mean_call: Option<Duration>,
    pub llm_mean_pass: Option<Duration>,
}

pub fn summarize(rows: &[Row], jev_calls: &[Duration], llm_passes: &[Duration]) -> Summary {
    let ok: Vec<&Row> = rows.iter().filter(|r| r.p_jev.is_some()).collect();
    let llm_rows: Vec<&Row> = rows.iter().filter(|r| r.llm.is_some()).collect();
    let pos: Vec<f64> = ok.iter().filter(|r| r.label).map(|r| r.p_jev.unwrap()).collect();
    let neg: Vec<f64> = ok.iter().filter(|r| !r.label).map(|r| r.p_jev.unwrap()).collect();
    let risks: Vec<(&String, &String)> = ok.iter().filter_map(|r| Some((r.llm_risk.as_ref()?, r.jev_risk.as_ref()?))).collect();
    let mean = |d: &[Duration]| (!d.is_empty()).then(|| d.iter().sum::<Duration>() / d.len() as u32);
    Summary {
        llm: (!llm_rows.is_empty()).then(|| pr_of(llm_rows.iter().map(|r| (r.label, r.llm.unwrap())))),
        jev: pr_of(ok.iter().map(|r| (r.label, r.p_jev.unwrap() >= 0.5))),
        jev_auc: crate::jev_probe::auc(&pos, &neg),
        risk_agreement: (!risks.is_empty()).then(|| risks.iter().filter(|(a, b)| a == b).count() as f64 / risks.len() as f64),
        disagreements: ok
            .iter()
            .filter(|r| (r.p_jev.unwrap() >= 0.5) != r.label)
            .map(|r| format!("{} {}: label {} · Jev P(relevant) {:.2}", r.source, r.path, if r.label { "RELEVANT" } else { "NOT_RELEVANT" }, r.p_jev.unwrap()))
            .collect(),
        errors: rows.iter().filter(|r| r.error.is_some()).count(),
        jev_mean_call: mean(jev_calls),
        llm_mean_pass: mean(llm_passes),
    }
}

pub fn render_text(title: &str, s: &Summary, rows: &[Row]) -> String {
    use std::fmt::Write;
    let mut out = format!("\n{title}\n");
    if let Some(l) = &s.llm {
        let _ = writeln!(out, "LLM  precision {:.2} · recall {:.2} · accuracy {:.2} (n={})", l.precision, l.recall, l.accuracy, l.n);
    }
    let _ = writeln!(out, "Jev  precision {:.2} · recall {:.2} · accuracy {:.2} (n={}) · AUC {:.2}", s.jev.precision, s.jev.recall, s.jev.accuracy, s.jev.n, s.jev_auc);
    if let Some(a) = s.risk_agreement {
        let _ = writeln!(out, "risk level: Jev agrees with the LLM on {:.0}%", a * 100.0);
    }
    if let (Some(j), Some(l)) = (s.jev_mean_call, s.llm_mean_pass) {
        let _ = writeln!(out, "time: Jev {:.1}s per file · LLM {:.1}s per PR", j.as_secs_f64(), l.as_secs_f64());
    } else if let Some(j) = s.jev_mean_call {
        let _ = writeln!(out, "time: Jev {:.1}s per file", j.as_secs_f64());
    }
    let _ = writeln!(out, "disagreements ({}):", s.disagreements.len());
    for d in &s.disagreements {
        let _ = writeln!(out, "    {d}");
    }
    for r in rows.iter().filter(|r| r.error.is_some()) {
        let _ = writeln!(out, "    ERROR {} {}: {}", r.source, r.path, r.error.as_deref().unwrap_or(""));
    }
    out
}

// ── jev-agree: Jev vs the LLM's cached calls on real PRs ─────────────────

#[derive(Deserialize)]
struct CachedManifest {
    pr_url: String,
    pr_title: String,
    #[serde(default)]
    body: String,
    files: Vec<CachedFile>,
}

#[derive(Deserialize)]
struct CachedFile {
    path: String,
    classification: String,
    #[serde(default)]
    risk_level: String,
    #[serde(default)]
    unified_diff: String,
}

/// `repo` is required on purpose: this sends each file's diff to TypeSafe,
/// so the caller names which repositories' code may go.
pub async fn agree(dir: &Path, repo: &str, limit: usize, json: bool) -> Result<(), String> {
    let key = resolve_jev_api_key(&load_settings()).ok_or("jev-agree needs a TypeSafe API key: set it in Settings or TYPESAFE_API_KEY")?;
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json") && !p.to_string_lossy().ends_with(".meta.json"))
        .collect();
    paths.sort();
    let mut rows = Vec::new();
    let mut calls = Vec::new();
    'outer: for p in paths {
        let Ok(m) = serde_json::from_str::<CachedManifest>(&std::fs::read_to_string(&p).unwrap_or_default()) else { continue };
        if !m.pr_url.contains(&format!("github.com/{repo}/")) {
            continue;
        }
        let pr = m.pr_url.rsplit('/').next().unwrap_or("").to_string();
        let all: Vec<String> = m.files.iter().map(|f| f.path.clone()).collect();
        for f in &m.files {
            if rows.len() >= limit {
                break 'outer;
            }
            let started = Instant::now();
            let res = classify_file(&key, &m.pr_title, &m.body, &f.path, &all, &f.unified_diff).await;
            calls.push(started.elapsed());
            let label = f.classification == "RELEVANT";
            rows.push(match res {
                Ok(j) => Row {
                    source: format!("#{pr}"),
                    path: f.path.clone(),
                    label,
                    llm: None,
                    p_jev: Some(j.p_relevant),
                    llm_risk: Some(f.risk_level.clone()),
                    jev_risk: Some(if j.relevant() { j.risk } else { "low".into() }),
                    error: None,
                },
                Err(e) => Row { source: format!("#{pr}"), path: f.path.clone(), label, llm: None, p_jev: None, llm_risk: None, jev_risk: None, error: Some(e) },
            });
            eprint!(".");
            tokio::time::sleep(JEV_SPACING).await;
        }
    }
    eprintln!();
    let s = summarize(&rows, &calls, &[]);
    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "summary": s, "rows": rows })).unwrap());
    } else {
        print!("{}", render_text(&format!("JEV vs the LLM's cached calls · {repo} · {} files (label = the LLM's call)", rows.len()), &s, &rows));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(label: bool, llm: Option<bool>, p: Option<f64>) -> Row {
        Row { source: "fx".into(), path: "a".into(), label, llm, p_jev: p, llm_risk: None, jev_risk: None, error: None }
    }

    #[test]
    fn precision_and_recall_count_relevant_as_positive() {
        let p = pr_of([(true, true), (true, false), (false, true), (false, false)].into_iter());
        assert_eq!((p.precision, p.recall, p.accuracy, p.n), (0.5, 0.5, 0.5, 4));
    }

    #[test]
    fn the_summary_scores_jev_and_the_llm_on_the_same_files() {
        let rows = vec![row(true, Some(true), Some(0.9)), row(false, Some(false), Some(0.7)), row(false, Some(true), Some(0.1))];
        let s = summarize(&rows, &[Duration::from_secs(1), Duration::from_secs(3)], &[Duration::from_secs(10)]);
        assert_eq!(s.jev.accuracy, 2.0 / 3.0);
        assert_eq!(s.llm.as_ref().unwrap().precision, 0.5);
        assert_eq!(s.disagreements.len(), 1);
        assert_eq!(s.jev_mean_call, Some(Duration::from_secs(2)));
    }

    #[test]
    fn failed_calls_are_counted_and_left_out_of_the_scores() {
        let mut bad = row(true, None, None);
        bad.error = Some("HTTP 529".into());
        let s = summarize(&[bad, row(true, None, Some(0.8))], &[], &[]);
        assert_eq!((s.errors, s.jev.n), (1, 1));
        assert!(s.llm.is_none());
    }
}
