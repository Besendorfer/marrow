//! `marrow eval --jev` (issue #249): how Jev's second opinion on each finding
//! lines up with the corpus labels. Pure summarizing and rendering; the eval
//! loop does the judging.
//!
//! The number that matters most is **false clears**: a finding the corpus
//! marks important that Jev calls noise or not real. A wrong "noise" is worse
//! than no judgement, so a Jev integration is only worth shipping if this
//! stays at (or very near) zero.

use marrow_core::jev::FindingJudgement;
use serde::Serialize;

/// One highlight the review produced, with its corpus label and Jev's call.
#[derive(Debug, Clone, Serialize)]
pub struct Judged {
    pub fixture: String,
    pub path: String,
    pub lines: String,
    pub category: String,
    /// "important" | "minor" | "should_not_flag" | "unlabeled"
    pub label: &'static str,
    pub judgement: Result<FindingJudgement, String>,
}

pub const LABELS: [&str; 4] = ["important", "minor", "should_not_flag", "unlabeled"];

/// Jev "clears" a finding when it calls it noise or more likely not real.
pub fn clears(j: &FindingJudgement) -> bool {
    j.urgency == "noise" || j.p_real < 0.5
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct Bucket {
    pub label: &'static str,
    pub n: usize,
    pub errors: usize,
    pub mean_p_real: f64,
    pub fix: usize,
    pub look: usize,
    pub noise: usize,
    pub cleared: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub buckets: Vec<Bucket>,
    /// Important findings Jev cleared — must be ~0.
    pub false_clears: Vec<String>,
    /// should_not_flag findings Jev cleared — the noise it would catch.
    pub noise_caught: usize,
    pub noise_total: usize,
    /// Each distinct failure message and how many calls hit it, so a run
    /// that failed (a bad key, a rate limit) says why.
    pub errors: Vec<(String, usize)>,
}

pub fn summarize(judged: &[Judged]) -> Summary {
    let mut s = Summary::default();
    for j in judged {
        if let Err(e) = &j.judgement {
            match s.errors.iter_mut().find(|(m, _)| m == e) {
                Some((_, n)) => *n += 1,
                None => s.errors.push((e.clone(), 1)),
            }
        }
    }
    for label in LABELS {
        let mut b = Bucket { label, ..Default::default() };
        let mut p_sum = 0.0;
        for j in judged.iter().filter(|j| j.label == label) {
            b.n += 1;
            match &j.judgement {
                Err(_) => b.errors += 1,
                Ok(v) => {
                    p_sum += v.p_real;
                    match v.urgency.as_str() {
                        "fix_before_merge" => b.fix += 1,
                        "worth_a_look" => b.look += 1,
                        _ => b.noise += 1,
                    }
                    if clears(v) {
                        b.cleared += 1;
                        if label == "important" {
                            s.false_clears.push(format!(
                                "{} {}:{} ({}) — {} · P(real) {:.2}",
                                j.fixture, j.path, j.lines, j.category, v.urgency, v.p_real
                            ));
                        }
                    }
                }
            }
        }
        let judged_n = b.n - b.errors;
        b.mean_p_real = if judged_n > 0 { p_sum / judged_n as f64 } else { 0.0 };
        if label == "should_not_flag" {
            s.noise_caught = b.cleared;
            s.noise_total = judged_n;
        }
        s.buckets.push(b);
    }
    s
}

pub fn render_text(s: &Summary) -> String {
    use std::fmt::Write;
    let mut out = String::from("\nJEV second opinion (TypeSafe jev-latest) by corpus label\n");
    let _ = writeln!(out, "{:<16} {:>3} {:>10} {:>5} {:>5} {:>6} {:>8} {:>7}", "label", "n", "P(real)", "fix", "look", "noise", "cleared", "errors");
    for b in s.buckets.iter().filter(|b| b.n > 0) {
        let _ = writeln!(
            out,
            "{:<16} {:>3} {:>10.2} {:>5} {:>5} {:>6} {:>8} {:>7}",
            b.label, b.n, b.mean_p_real, b.fix, b.look, b.noise, b.cleared, b.errors
        );
    }
    let _ = writeln!(out, "FALSE CLEARS (important called noise / not real): {}", s.false_clears.len());
    for f in &s.false_clears {
        let _ = writeln!(out, "    {f}");
    }
    let _ = writeln!(out, "NOISE CAUGHT (should-not-flag cleared): {}/{}", s.noise_caught, s.noise_total);
    for (msg, n) in &s.errors {
        let _ = writeln!(out, "JEV ERROR ×{n}: {msg}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn j(label: &'static str, urgency: &str, p_real: f64) -> Judged {
        Judged {
            fixture: "fx".into(),
            path: "a.rs".into(),
            lines: "1-2".into(),
            category: "bug".into(),
            label,
            judgement: Ok(FindingJudgement {
                p_real,
                defect_confidence: 0.5,
                urgency: urgency.into(),
                p_fix: 0.0,
                p_look: 0.0,
                p_noise: 0.0,
                urgency_confidence: 0.5,
            }),
        }
    }

    #[test]
    fn an_important_finding_called_noise_or_unreal_is_a_false_clear() {
        let s = summarize(&[
            j("important", "fix_before_merge", 0.9),
            j("important", "noise", 0.8),
            j("important", "worth_a_look", 0.3),
        ]);
        assert_eq!(s.false_clears.len(), 2);
        let imp = &s.buckets[0];
        assert_eq!((imp.n, imp.fix, imp.look, imp.noise, imp.cleared), (3, 1, 1, 1, 2));
        assert!((imp.mean_p_real - (0.9 + 0.8 + 0.3) / 3.0).abs() < 1e-9);
    }

    #[test]
    fn clearing_should_not_flag_findings_counts_as_noise_caught() {
        let s = summarize(&[j("should_not_flag", "noise", 0.2), j("should_not_flag", "worth_a_look", 0.7)]);
        assert_eq!((s.noise_caught, s.noise_total), (1, 2));
        assert!(s.false_clears.is_empty());
    }

    #[test]
    fn a_failed_judgement_is_counted_but_not_averaged() {
        let mut bad = j("minor", "noise", 0.0);
        bad.judgement = Err("HTTP 500".into());
        let s = summarize(&[bad, j("minor", "worth_a_look", 0.6)]);
        let minor = &s.buckets[1];
        assert_eq!((minor.n, minor.errors), (2, 1));
        assert!((minor.mean_p_real - 0.6).abs() < 1e-9);
        assert_eq!(s.errors, vec![("HTTP 500".to_string(), 1)]);
        assert!(render_text(&s).contains("JEV ERROR ×1: HTTP 500"));
    }
}
