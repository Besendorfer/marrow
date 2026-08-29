//! Analysis-environment fingerprint (issue #202): a stable hash of
//! everything that shapes an analysis BESIDES the PR's own content — the
//! pipeline version, the analysis prompts, the prompt budgets, and the
//! provider/model settings. Stamped into each cached manifest so a cache
//! produced by a different model, an edited prompt, or an older pipeline no
//! longer silently presents as current. PR-content freshness (`head_sha`)
//! stays the existing update-check's job; the two together define validity.

use crate::budgets;
use crate::prompts;
use crate::types::Settings;
use sha2::{Digest, Sha256};

/// Bump when the analysis pipeline changes in a way the hashed inputs can't
/// see (pass structure, parsing rules, manifest semantics).
pub const PIPELINE_VERSION: u32 = 1;

/// Fingerprint of the current analysis environment. Prompts are hashed
/// verbatim, so editing one auto-invalidates caches with no manual bump.
/// Settings contribute only the analysis-relevant, non-secret fields.
pub fn analysis_fingerprint(settings: &Settings) -> String {
    fingerprint_of(
        PIPELINE_VERSION,
        &[
            prompts::CLASSIFICATION_PROMPT,
            prompts::HIGHLIGHT_PROMPT,
            prompts::SUMMARY_PROMPT,
            prompts::GROUPING_PROMPT,
            prompts::TRIAGE_PROMPT,
            prompts::REQUIREMENTS_COVERAGE_PROMPT,
        ],
        &[
            budgets::CLASSIFICATION_DIFF,
            budgets::HIGHLIGHT_PER_FILE,
            budgets::HIGHLIGHT_TOTAL,
            budgets::HIGHLIGHT_BODY,
            budgets::TRIAGE_PER_FILE,
            budgets::TRIAGE_TOTAL,
            budgets::COVERAGE_PER_FILE,
            budgets::COVERAGE_TOTAL,
            budgets::COVERAGE_EXISTING_PER_FILE,
            budgets::COVERAGE_EXISTING_TOTAL,
            budgets::COVERAGE_ISSUE_PER_ISSUE,
            budgets::COVERAGE_ISSUE_TOTAL,
            budgets::COVERAGE_BODY,
            budgets::COVERAGE_USER_REQS,
        ],
        settings,
    )
}

/// The hash itself, input-injectable so tests can vary prompts/budgets —
/// the production inputs are constants.
fn fingerprint_of(version: u32, prompts: &[&str], budgets: &[usize], settings: &Settings) -> String {
    let mut h = Sha256::new();
    h.update(version.to_le_bytes());
    for prompt in prompts {
        h.update(prompt.as_bytes());
        h.update([0]); // unambiguous field boundary
    }
    for budget in budgets {
        h.update((*budget as u64).to_le_bytes());
    }
    for field in [&settings.provider, &settings.model, &settings.openai_base_url] {
        h.update(field.as_bytes());
        h.update([0]);
    }
    format!("{:x}", h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(model: &str, provider: &str, base: &str) -> Settings {
        Settings {
            model: model.to_string(),
            provider: provider.to_string(),
            openai_base_url: base.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn stable_for_identical_inputs() {
        let a = analysis_fingerprint(&settings("claude-sonnet-4-6", "", ""));
        let b = analysis_fingerprint(&settings("claude-sonnet-4-6", "", ""));
        assert_eq!(a, b);
    }

    #[test]
    fn changes_with_model_provider_or_base_url() {
        let base = analysis_fingerprint(&settings("m1", "", ""));
        assert_ne!(base, analysis_fingerprint(&settings("m2", "", "")));
        assert_ne!(base, analysis_fingerprint(&settings("m1", "openai", "")));
        assert_ne!(base, analysis_fingerprint(&settings("m1", "", "http://localhost:1234")));
    }

    #[test]
    fn secrets_do_not_contribute() {
        let mut with_key = settings("m1", "", "");
        with_key.anthropic_api_key = "sk-secret".to_string();
        with_key.github_token = "ghp_secret".to_string();
        assert_eq!(
            analysis_fingerprint(&settings("m1", "", "")),
            analysis_fingerprint(&with_key),
            "keys/tokens must not affect (or leak into) the fingerprint"
        );
    }

    #[test]
    fn prompt_budget_and_version_changes_invalidate() {
        let s = settings("m", "", "");
        let base = fingerprint_of(1, &["prompt A", "prompt B"], &[100, 200], &s);
        // Editing a prompt auto-invalidates — no manual bump needed.
        assert_ne!(base, fingerprint_of(1, &["prompt A edited", "prompt B"], &[100, 200], &s));
        // Retuning a budget invalidates.
        assert_ne!(base, fingerprint_of(1, &["prompt A", "prompt B"], &[100, 250], &s));
        // A pipeline bump invalidates even with identical prompts/budgets.
        assert_ne!(base, fingerprint_of(2, &["prompt A", "prompt B"], &[100, 200], &s));
    }

    #[test]
    fn field_boundaries_are_unambiguous() {
        // "ab" + "c" must not collide with "a" + "bc".
        assert_ne!(
            analysis_fingerprint(&settings("ab", "c", "")),
            analysis_fingerprint(&settings("a", "bc", ""))
        );
    }
}
