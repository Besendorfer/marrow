//! `marrow eval --jev-probe` (issue #249, round 2): can Jev tell a real claim
//! from a counterfeit one? Runs every probe in `corpus/jev-probes.json` —
//! reviewer-style claims about a fixture's diff, labeled real or counterfeit —
//! through each question set and reports how well P(real) separates them.
//! No review calls: it measures Jev alone, independent of how noisy the
//! review model happens to be.

use marrow_core::config::{load_settings, resolve_jev_api_key};
use marrow_core::jev::{evaluate, finding_questions, finding_questions_v2, finding_state_with_evidence, JevChoice, Q_DEFECT, Q_INTENDED, Q_URGENCY};
use marrow_core::types::HighlightResult;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Probe {
    pub fixture: String,
    pub path: String,
    pub start_line: u64,
    pub end_line: u64,
    pub category: String,
    /// "real" | "counterfeit"
    pub truth: String,
    /// real | invented | wrong_line | intended | trivial
    pub kind: String,
    pub claim: String,
    /// A real claim only verifiable from code outside the diff Jev sees.
    #[serde(default)]
    pub outside_diff: bool,
}

#[derive(Deserialize)]
struct ProbeFile {
    /// Per fixture: repo-snapshot files (relative to the fixture dir) an
    /// agentic review would have read; given to the judge in the
    /// with-evidence run for every probe on that fixture.
    #[serde(default)]
    evidence: BTreeMap<String, Vec<String>>,
    probes: Vec<Probe>,
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

/// One probe's answers under one question set.
#[derive(Debug, Clone, Serialize)]
pub struct Scored {
    pub probe: usize,
    pub p_real: f64,
    pub p_noise: f64,
    pub p_intended: Option<f64>,
}

#[derive(Debug, Default, Serialize)]
pub struct VariantReport {
    pub variant: String,
    pub errors: Vec<String>,
    /// Mean P(real) by probe kind (outside-diff real claims listed apart).
    pub mean_p_real: BTreeMap<String, f64>,
    /// P(a random in-diff real claim outranks a random counterfeit) on P(real).
    pub auc: f64,
    /// Share of in-diff real + counterfeit probes on the right side of 0.5.
    pub accuracy: f64,
    /// Same, on P(real) × (1 − P(intended)) — v2 only.
    pub auc_adjusted: Option<f64>,
    pub accuracy_adjusted: Option<f64>,
    pub scored: Vec<Scored>,
}

fn kind_key(p: &Probe) -> String {
    if p.outside_diff { format!("{} (outside diff)", p.kind) } else { p.kind.clone() }
}

/// P(real score of a real probe > that of a counterfeit), ties count half.
pub fn auc(real: &[f64], fake: &[f64]) -> f64 {
    if real.is_empty() || fake.is_empty() {
        return 0.0;
    }
    let mut wins = 0.0;
    for r in real {
        for f in fake {
            wins += if r > f { 1.0 } else if r == f { 0.5 } else { 0.0 };
        }
    }
    wins / (real.len() * fake.len()) as f64
}

fn accuracy(real: &[f64], fake: &[f64]) -> f64 {
    let n = real.len() + fake.len();
    if n == 0 {
        return 0.0;
    }
    let right = real.iter().filter(|s| **s >= 0.5).count() + fake.iter().filter(|s| **s < 0.5).count();
    right as f64 / n as f64
}

/// Score one variant's answers. Outside-diff real claims are reported but
/// left out of AUC/accuracy — the judge can't see what makes them true.
pub fn score_variant(variant: &str, probes: &[Probe], scored: Vec<Scored>, errors: Vec<String>) -> VariantReport {
    let mut sums: BTreeMap<String, (f64, usize)> = BTreeMap::new();
    for s in &scored {
        let e = sums.entry(kind_key(&probes[s.probe])).or_default();
        e.0 += s.p_real;
        e.1 += 1;
    }
    let split = |f: &dyn Fn(&Scored) -> f64| {
        let real: Vec<f64> = scored.iter().filter(|s| probes[s.probe].truth == "real" && !probes[s.probe].outside_diff).map(f).collect();
        let fake: Vec<f64> = scored.iter().filter(|s| probes[s.probe].truth == "counterfeit").map(f).collect();
        (real, fake)
    };
    let (real, fake) = split(&|s| s.p_real);
    let has_intended = scored.iter().any(|s| s.p_intended.is_some());
    let adjusted = has_intended.then(|| split(&|s| s.p_real * (1.0 - s.p_intended.unwrap_or(0.0))));
    VariantReport {
        variant: variant.to_string(),
        errors,
        mean_p_real: sums.into_iter().map(|(k, (sum, n))| (k, sum / n as f64)).collect(),
        auc: auc(&real, &fake),
        accuracy: accuracy(&real, &fake),
        auc_adjusted: adjusted.as_ref().map(|(r, f)| auc(r, f)),
        accuracy_adjusted: adjusted.as_ref().map(|(r, f)| accuracy(r, f)),
        scored,
    }
}

fn prob(a: &BTreeMap<String, JevChoice>, q: &str, label: &str) -> Option<f64> {
    a.get(q).and_then(|c| c.probabilities.get(label).copied())
}

pub async fn run(corpus: &Path, json: bool) -> Result<(), String> {
    let key = resolve_jev_api_key(&load_settings())
        .ok_or("--jev-probe needs a TypeSafe API key: set it in Settings or TYPESAFE_API_KEY")?;
    let file: ProbeFile = serde_json::from_str(
        &std::fs::read_to_string(corpus.join("jev-probes.json")).map_err(|e| format!("Failed to read jev-probes.json: {e}"))?,
    )
    .map_err(|e| format!("jev-probes.json: {e}"))?;
    let probes = file.probes;
    let mut evidence: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (fixture, paths) in &file.evidence {
        for rel in paths {
            let content = std::fs::read_to_string(corpus.join("fixtures").join(fixture).join(rel))
                .map_err(|e| format!("evidence {fixture}/{rel}: {e}"))?;
            // Show the judge the repo path, not the snapshot layout.
            let shown = rel.splitn(3, '/').nth(2).unwrap_or(rel).to_string();
            evidence.entry(fixture.clone()).or_default().push((shown, content));
        }
    }
    // Load and check every fixture before the first call.
    let mut prs: BTreeMap<String, FixturePr> = BTreeMap::new();
    for p in &probes {
        if !prs.contains_key(&p.fixture) {
            let raw = std::fs::read_to_string(corpus.join("fixtures").join(&p.fixture).join("pr.json"))
                .map_err(|e| format!("probe fixture {}: {e}", p.fixture))?;
            prs.insert(p.fixture.clone(), serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", p.fixture))?);
        }
        if !prs[&p.fixture].files.iter().any(|f| f.path == p.path) {
            return Err(format!("probe {}:{} names a file not in the fixture", p.fixture, p.path));
        }
        if p.truth != "real" && p.truth != "counterfeit" {
            return Err(format!("probe {}:{}: truth must be real or counterfeit", p.fixture, p.path));
        }
    }
    eprintln!("{} probe(s) · 3 runs · TypeSafe jev-latest", probes.len());

    // v2+evidence: the v2 questions, plus the outside-diff files the review read.
    let variants: [(&str, BTreeMap<_, _>, bool); 3] = [
        ("v1", finding_questions(), false),
        ("v2", finding_questions_v2(), false),
        ("v2+evidence", finding_questions_v2(), true),
    ];
    let no_evidence: Vec<(String, String)> = Vec::new();
    let mut reports = Vec::new();
    for (name, questions, with_evidence) in variants {
        let mut scored = Vec::new();
        let mut errors = Vec::new();
        for (i, p) in probes.iter().enumerate() {
            let pr = &prs[&p.fixture];
            let diff = pr.files.iter().find(|f| f.path == p.path).map(|f| f.diff.as_str()).unwrap_or("");
            // Only the claim varies: no scenario or fix, and the same severity
            // for every defect claim, so nothing but the claim itself leaks truth.
            let h = HighlightResult {
                path: p.path.clone(),
                start_line: p.start_line,
                end_line: p.end_line,
                severity: if p.category == "simplification" { "info".into() } else { "warning".into() },
                comment: p.claim.clone(),
                category: p.category.clone(),
                scenario: String::new(),
                fix: String::new(),
            };
            let ev = if with_evidence { evidence.get(&p.fixture).unwrap_or(&no_evidence) } else { &no_evidence };
            match evaluate(&key, &finding_state_with_evidence(&pr.title, &pr.body, &h, diff, ev), &questions).await {
                Ok(a) => scored.push(Scored {
                    probe: i,
                    p_real: prob(&a, Q_DEFECT, "real").unwrap_or(0.0),
                    p_noise: prob(&a, Q_URGENCY, "noise").unwrap_or(0.0),
                    p_intended: prob(&a, Q_INTENDED, "true"),
                }),
                Err(e) => errors.push(format!("{} {}:{}-{}: {e}", p.fixture, p.path, p.start_line, p.end_line)),
            }
            eprint!(".");
            tokio::time::sleep(marrow_core::jev::CALL_SPACING).await;
        }
        eprintln!(" {name} done");
        reports.push(score_variant(name, &probes, scored, errors));
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "probes": probes, "reports": reports })).unwrap());
    } else {
        print!("{}", render_text(&probes, &reports));
    }
    Ok(())
}

pub fn render_text(probes: &[Probe], reports: &[VariantReport]) -> String {
    use std::fmt::Write;
    let mut out = String::from("\nJEV PROBE: real vs counterfeit claims\n");
    for r in reports {
        let _ = writeln!(out, "\n[{}] AUC {:.2} · accuracy@0.5 {:.2}", r.variant, r.auc, r.accuracy);
        if let (Some(a), Some(c)) = (r.auc_adjusted, r.accuracy_adjusted) {
            let _ = writeln!(out, "[{}] with P(intended): AUC {:.2} · accuracy@0.5 {:.2}", r.variant, a, c);
        }
        for (k, v) in &r.mean_p_real {
            let _ = writeln!(out, "    mean P(real) {:<26} {:.2}", k, v);
        }
        for s in &r.scored {
            let p = &probes[s.probe];
            let intended = s.p_intended.map(|v| format!(" intended {v:.2}")).unwrap_or_default();
            let _ = writeln!(
                out,
                "    {:<11} {:<11} real {:.2} noise {:.2}{}  {} {}:{}",
                p.truth, p.kind, s.p_real, s.p_noise, intended, p.fixture, p.path, p.start_line
            );
        }
        for e in &r.errors {
            let _ = writeln!(out, "    ERROR {e}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(truth: &str, kind: &str, outside: bool) -> Probe {
        Probe {
            fixture: "fx".into(),
            path: "a.rs".into(),
            start_line: 1,
            end_line: 1,
            category: "bug".into(),
            truth: truth.into(),
            kind: kind.into(),
            claim: "c".into(),
            outside_diff: outside,
        }
    }

    #[test]
    fn auc_is_the_share_of_real_over_counterfeit_pairs() {
        assert_eq!(auc(&[0.9, 0.8], &[0.1, 0.2]), 1.0);
        assert_eq!(auc(&[0.1], &[0.9]), 0.0);
        assert_eq!(auc(&[0.5], &[0.5]), 0.5);
        assert!((auc(&[0.9, 0.3], &[0.5]) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn outside_diff_real_claims_stay_out_of_the_separation_score() {
        let probes = vec![probe("real", "real", false), probe("real", "real", true), probe("counterfeit", "invented", false)];
        let scored = vec![
            Scored { probe: 0, p_real: 0.9, p_noise: 0.0, p_intended: None },
            Scored { probe: 1, p_real: 0.1, p_noise: 0.0, p_intended: None },
            Scored { probe: 2, p_real: 0.4, p_noise: 0.0, p_intended: None },
        ];
        let r = score_variant("v1", &probes, scored, vec![]);
        assert_eq!(r.auc, 1.0);
        assert_eq!(r.accuracy, 1.0);
        assert!(r.mean_p_real.contains_key("real (outside diff)"));
        assert!(r.auc_adjusted.is_none());
    }

    #[test]
    fn p_intended_adjusts_the_score_when_present() {
        let probes = vec![probe("real", "real", false), probe("counterfeit", "intended", false)];
        let scored = vec![
            Scored { probe: 0, p_real: 0.8, p_noise: 0.0, p_intended: Some(0.1) },
            Scored { probe: 1, p_real: 0.9, p_noise: 0.0, p_intended: Some(0.9) },
        ];
        let r = score_variant("v2", &probes, scored, vec![]);
        assert_eq!(r.auc, 0.0); // on P(real) alone the counterfeit wins
        assert_eq!(r.auc_adjusted, Some(1.0)); // 0.72 vs 0.09
    }

    #[test]
    fn the_corpus_probe_file_parses_and_names_real_fixture_files() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../corpus");
        let file: ProbeFile = serde_json::from_str(&std::fs::read_to_string(dir.join("jev-probes.json")).unwrap()).unwrap();
        assert!(file.probes.iter().any(|p| p.truth == "real") && file.probes.iter().any(|p| p.truth == "counterfeit"));
        for p in &file.probes {
            let pr: FixturePr = serde_json::from_str(&std::fs::read_to_string(dir.join("fixtures").join(&p.fixture).join("pr.json")).unwrap()).unwrap();
            assert!(pr.files.iter().any(|f| f.path == p.path), "{}:{}", p.fixture, p.path);
            assert!(p.truth == "real" || p.truth == "counterfeit");
        }
    }
}
