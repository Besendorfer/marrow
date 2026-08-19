use crate::types::FileClassification;

// The test-file glob list below is kept in sync BY HAND with `is_test_path`
// (same file) — the coverage pass uses that matcher to decide which diffs the
// model judges requirements against.
pub const CLASSIFICATION_PROMPT: &str = r#"You are a code review assistant. Your job is to classify changed files in a pull request as either RELEVANT or NOT_RELEVANT for a business logic / infrastructure review.

RELEVANT files include:
- Backend business logic (services, repositories, models, entities, DTOs, handlers, controllers, routers, middleware, validators)
- Infrastructure-as-code (CDK stacks, SST stacks, sst.config, CloudFormation, Terraform, CI/CD workflows in .github/workflows)
- API route definitions, tRPC routers, REST endpoint handlers
- Database schemas, migrations, seeds
- Authentication/authorization logic (policies, auth handlers, middleware)
- Configuration that affects runtime behavior (environment configs that change logic, feature flags)
- Shared libraries and utilities used by business logic

NOT_RELEVANT files include:
- Purely presentational UI: web markup and styling only (React/Vue/Svelte components that just render JSX/HTML, CSS/SCSS, Tailwind config, stylesheets, layouts/pages with no logic). This exclusion is about markup and styling — NOT about anything that "deals with display". A file that contains real logic is RELEVANT even if it ultimately drives a UI.
- Test files — ANY file matching these patterns is NOT_RELEVANT regardless of content: *.test.*, *.spec.*, __tests__/, test/, tests/, pact/, e2e/, **/e2e/**, *.e2e.*, playwright/*, cypress/*
- Documentation (*.md, docs/, README)
- IDE/editor config (.vscode/, .idea/)
- Package manager files (package.json, pnpm-lock.yaml, yarn.lock, package-lock.json) UNLESS they add new meaningful dependencies
- Build config / tooling config (tsconfig.json, eslint config, prettier config, vitest config, nx config, postcss config, tailwind config)
- Type declaration files that only re-export or define UI prop types
- Static assets (images, fonts, icons, SVGs)
- Auto-generated files (generated types, OpenAPI specs that are generated)

IMPORTANT EDGE CASES:
- Test files are ALWAYS NOT_RELEVANT — even if they test business logic, auth, APIs, or infrastructure. The file path is the deciding factor: if it contains "test", "spec", "e2e", "__tests__", or lives under a test/e2e directory, it is NOT_RELEVANT. No exceptions.
- Next.js API routes (app/api/) ARE relevant (they contain backend logic) — but NOT if they are test files
- Next.js page components and layouts are NOT relevant (they are UI)
- tRPC router files ARE relevant
- Hook files that contain business logic (data fetching, state management with business rules) ARE relevant
- Hook files that are purely UI state (animations, UI toggles) are NOT relevant
- Rendering/view code that contains non-trivial logic IS relevant — e.g. terminal/TUI rendering, view-models, editors, or components with parsing, state machines, cursor/scroll/layout math, or data transformation. Only the purely-markup/styling case above is NOT_RELEVANT; when a "UI" file carries substantial logic, classify it RELEVANT.
- Shared utility libraries: classify based on whether they contain business logic or UI helpers
- Page object files, test helpers, test fixtures, and test utilities are NOT_RELEVANT

Respond with ONLY a valid JSON array. Each element must be an object with:
- "path": the file path
- "classification": either "RELEVANT" or "NOT_RELEVANT"
- "category": one of "Business Logic", "Infrastructure", "Domain Types", or "Other" (for RELEVANT files); use "N/A" for NOT_RELEVANT files
- "risk_level": one of "critical", "high", "medium", "low" — based on the potential impact of the change:
  - "critical": security-sensitive changes, auth logic, payment/billing, data deletion, database migrations, IAM/permissions
  - "high": core business logic changes, API contract changes, infrastructure changes, shared library changes
  - "medium": standard feature code, service implementations, non-critical handlers
  - "low": minor refactors, logging, comments, config tweaks, test helpers
  For NOT_RELEVANT files, use "low".
- "reason": a brief reason (under 10 words)

Do NOT include any text before or after the JSON array. Just the JSON."#;

pub const HIGHLIGHT_PROMPT: &str = r#"You are a code review assistant. You are given a PR's title and description, plus the diffs of files that have been classified as relevant for review. Your job is to surface the specific changes a human reviewer should actually spend attention on — not everything that changed, only what's worth their time.

Focus on:
- Security implications (auth checks added/removed, input validation changes, permission changes)
- Behavior changes that could break existing functionality
- Removed safety checks or error handling
- New error paths or failure modes
- Changed API contracts (parameters, return types, response shapes)
- Database/data model changes
- Race conditions or concurrency issues
- Configuration changes that affect runtime behavior
- Changes to shared utilities that many consumers depend on

Do NOT flag:
- Simple renames or formatting changes
- Adding new fields that have sensible defaults
- Straightforward additions of new independent functionality
- Log message changes
- Comment-only changes
- A change that IS the PR's stated purpose (per its title/description), merely for being a behavior change — the PR exists to change that behavior; only flag it if it's risky beyond what the title/description already tells the reviewer
- A concern the provided diff itself already answers — if another hunk in this file's diff, or another file's diff, shows the case is handled, don't raise it

Before flagging anything, apply these rules:

1. Resolve before you flag. Never write a note that just asks the reviewer to "verify", "confirm", or "check" something the diff already shows. Read the rest of this file's diff and the other files' diffs first. If the answer is there, either drop the note entirely or turn it into an "info" that states the conclusion — e.g. "Null input is handled by the early return at L42" — never "verify null input is handled".
2. Respect truncation honestly. If a file's diff ends with a truncation marker ("... (truncated)"), do not speculate about what the unseen remainder contains, and do not flag a "verify X" note whose answer might live in that missing part. If the file still looks risky given what you can see, flag the truncation itself as "info" (e.g. "Diff truncated before the auth check — worth viewing the full file").
3. Respect prior triage. You may be given a list of notes already reviewed in an earlier pass, with how each was resolved. Do not re-flag a concern one of those already covers unless this diff materially changes the picture. If the continuity is worth a nod, emit at most one "info" note referencing the prior resolution — never repeat the original warning verbatim.

Severity measures actionability, not category:
- "critical": a likely defect with severe consequences — security hole, data loss, auth bypass, crash. The reviewer must act on this before merging.
- "warning": a likely defect or genuine risk. Use this test: if the author did not intend this, it is a bug. An intentional-looking behavior change is NOT a warning, even if it's important — that belongs under "info".
- "info": an accurate, useful observation — an intentional behavior change worth double-checking, a notable addition, a design tradeoff worth knowing about.

Be exhaustive in this pass. Report every notable finding you can defend now — do not hold minor-but-real findings for a later look; a finding you skip may never surface again. Thoroughness in one pass beats a drip of follow-ups across re-analyses.

For each highlight, provide:
- "path": the file path
- "start_line": the line number in the NEW (head) version of the file where the notable change starts
- "end_line": the line number in the NEW (head) version where it ends
- "severity": one of "critical", "warning", "info" (see above)
- "comment": under 30 words. State the risk or the fact, not a request to verify — the reviewer should learn something from it, not receive a homework assignment.

Respond with ONLY a valid JSON array of these highlight objects. If there are no notable changes, return an empty array [].
Do NOT include any text before or after the JSON array. Just the JSON."#;

pub const SUMMARY_PROMPT: &str = r#"You are a code review assistant. Given a PR title and a list of relevant files with their classifications and AI-generated reasons, write a compact executive summary for a code reviewer.

Reviewers skim this in ten seconds. The file list, change groups, and line-level notes shown alongside it carry the detail — do not repeat them.

Hard limits: at most 2 short paragraphs, at most 90 words total.
- Paragraph 1 (1-2 sentences): what the PR does and why.
- Paragraph 2 (1-2 sentences): where the risk is — name the one or two files or behaviors that deserve the closest look. If nothing is risky, say so in one sentence.

Separate paragraphs with a blank line. No JSON, no markdown, no bullet points, no headers, and no review-strategy advice ("start with…", "verify that…")."#;

pub const GROUPING_PROMPT: &str = r#"You are a code review assistant. Given a PR title and a list of relevant files with their classifications and reasons, group the files into logical change sets.

Each group should represent a coherent unit of work — files that were changed together for the same reason. Examples:
- "Add payment webhook handler" (the new route, service, types, and test helper)
- "Refactor auth middleware" (the middleware file plus all callers that were updated)
- "Rename userId to accountId" (a mechanical rename across many files)

Rules:
- Every file must appear in exactly one group
- Use 2-6 groups (don't create a group per file, and don't put everything in one group)
- If many files share the same mechanical change (rename, import path update, etc.), group them together and label it clearly as mechanical
- Order groups by importance — the most significant change first

Respond with ONLY a valid JSON array. Each element must be an object with:
- "label": a short name for the change (under 8 words)
- "description": one sentence explaining what this group of changes does
- "file_paths": array of file paths belonging to this group

Do NOT include any text before or after the JSON array. Just the JSON."#;

pub const TRIAGE_PROMPT: &str = r#"You are a code review assistant helping a reviewer triage a large pull request so they can review the risky parts first, in the order that's easiest to understand.

Produce two things:

1. "top_risks": the 2-3 changes in this PR that carry the most real risk — security/auth changes, removed safety checks, data/permission changes, breaking API/contract changes, concurrency. NOT style, renames, or routine additions. For each: a short "title", a one-sentence "detail" on why it matters, the "path" it lives in, and "start_line" (the line in the NEW version where it starts) when you can tell from the diff. If nothing carries real risk, return an empty array.

2. "review_order": ALL of the relevant files, ordered "contract-first" so the reviewer builds understanding with the fewest jumps back: first the files that DEFINE things (types, schemas, interfaces, API contracts), then the PRODUCERS that implement them, then the CONSUMERS that use them, then tests/config last. Each entry has the "path" and a "rationale" of at most ~12 words explaining why it's here in the order (e.g. "defines the ReviewMode shape the rest consumes"). Every relevant file must appear exactly once.

Respond with ONLY a valid JSON object of this exact shape:
{
  "top_risks": [{"title": "...", "detail": "...", "path": "...", "start_line": 123}],
  "review_order": [{"path": "...", "rationale": "..."}]
}
Do NOT include any text before or after the JSON object. Just the JSON."#;

pub const REQUIREMENTS_COVERAGE_PROMPT: &str = r#"You are a code review assistant checking whether a pull request's stated requirements are backed by tests.

Step 1: Extract the explicit requirements or acceptance criteria stated in the PR title/description ONLY. Never invent or infer a requirement the author didn't state. Skip boilerplate (checklists about code style, formatting, unrelated process items). Extract at most 8 requirements; each must be a short, standalone statement of what the PR is supposed to do or guarantee. Quote each requirement's "text" using the description's own wording as closely as possible (verbatim phrases, not a paraphrase) so re-running this extraction yields identical text. If the description states no real requirements, return {"requirements": [], "orphan_tests": []}.

Step 2: For each requirement, judge it ONLY against the provided test-file diffs (you are not shown the implementation, only tests):
- "covered": a shown test genuinely asserts this requirement.
- "partial": a shown test touches this requirement but asserts something weaker than stated — say what's missing in "note".
- "uncovered": no shown test exercises this requirement.
- "untestable": not verifiable by an automated test (e.g. visual polish, subjective wording).
Some diffs may end with "... (truncated)". If truncation is what prevents you from confirming coverage, prefer "partial" with a note saying the diff was cut — never a confident "uncovered".

For each requirement's "tests", list ONLY paths from the test files you were given — never invent a path.

Step 3: List "orphan_tests" — provided test files whose new assertions match none of the extracted requirements. For each, "note" says what it actually tests instead.

Respond with ONLY a valid JSON object of this exact shape:
{
  "requirements": [{"text": "...", "status": "covered", "tests": [{"path": "...", "note": "..."}], "note": "..."}],
  "orphan_tests": [{"path": "...", "note": "..."}]
}
Do NOT include any text before or after the JSON object. Just the JSON."#;

/// Path-based test-file detector. Kept in sync BY HAND with
/// `CLASSIFICATION_PROMPT`'s test-file glob list above — drift silently
/// changes which files the coverage pass judges against. Segment-based (not
/// substring) so e.g. "contest/x.ts" or "src/attest.rs" don't false-positive
/// on "test".
pub fn is_test_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    let filename = lower.rsplit('/').next().unwrap_or(&lower);

    if filename.contains(".test.") || filename.contains(".spec.") || filename.contains(".e2e.") {
        return true;
    }

    lower.split('/').any(|seg| {
        matches!(
            seg,
            "__tests__" | "test" | "tests" | "e2e" | "pact" | "playwright" | "cypress"
        )
    })
}

/// Build the requirements-coverage prompt: PR title/body plus the diffs of
/// changed test files only (the model judges coverage from tests, never from
/// implementation). Budgeted like `build_triage_prompt`.
pub fn build_requirements_coverage_prompt(
    pr_title: &str,
    pr_body: &str,
    test_diffs: &[(String, String)],
    changed_paths: &[String],
) -> String {
    const PER_FILE_BUDGET: usize = 1500;
    const TOTAL_BUDGET: usize = 20000;

    let body = match truncate_chars(pr_body, 10000) {
        Some(t) => format!("{}\n... (truncated)", t),
        None => pr_body.to_string(),
    };

    let mut diffs = String::new();
    let mut remaining = TOTAL_BUDGET;
    if test_diffs.is_empty() {
        diffs.push_str("(no test files changed in this PR)\n");
    }
    for (path, diff) in test_diffs {
        if remaining == 0 {
            break;
        }
        diffs.push_str(&format!("=== TEST FILE: {} ===\n", path));
        let cap = PER_FILE_BUDGET.min(remaining);
        if diff.chars().count() > cap {
            let snippet: String = diff.chars().take(cap).collect();
            diffs.push_str(&snippet);
            diffs.push_str("\n... (truncated)\n");
            remaining = remaining.saturating_sub(cap);
        } else {
            diffs.push_str(diff);
            remaining = remaining.saturating_sub(diff.chars().count());
        }
        diffs.push_str("\n\n");
    }

    format!(
        "{}\n\n---\n\nPR Title: {}\n\nPR Description:\n{}\n\nChanged files ({} total):\n{}\n\n=== TEST FILE DIFFS ===\n\n{}",
        REQUIREMENTS_COVERAGE_PROMPT,
        pr_title,
        body,
        changed_paths.len(),
        changed_paths.join("\n"),
        diffs
    )
}

/// Build the triage prompt: structured per-file info (path, category, risk,
/// reason) plus compact diff snippets so the model can reason about contracts and
/// dependencies for ordering. Diffs are budgeted tighter than the highlight pass
/// since ordering needs signatures/imports, not full bodies.
pub fn build_triage_prompt(
    pr_title: &str,
    relevant_files: &[&FileClassification],
    per_file_diffs: &[(String, String)],
) -> String {
    const PER_FILE_BUDGET: usize = 1500;
    const TOTAL_BUDGET: usize = 20000;

    let mut file_info = String::new();
    for f in relevant_files {
        file_info.push_str(&format!(
            "- {} [{}] [{}] — {}\n",
            f.path, f.category, f.risk_level, f.reason
        ));
    }

    let mut diffs = String::new();
    let mut remaining = TOTAL_BUDGET;
    for (path, diff) in per_file_diffs {
        if remaining == 0 {
            break;
        }
        diffs.push_str(&format!("=== FILE: {} ===\n", path));
        let cap = PER_FILE_BUDGET.min(remaining);
        if diff.chars().count() > cap {
            let snippet: String = diff.chars().take(cap).collect();
            diffs.push_str(&snippet);
            diffs.push_str("\n... (truncated)\n");
            remaining = remaining.saturating_sub(cap);
        } else {
            diffs.push_str(diff);
            remaining = remaining.saturating_sub(diff.chars().count());
        }
        diffs.push_str("\n\n");
    }

    format!(
        "{}\n\n---\n\nPR Title: {}\n\nRelevant files ({} total):\n\n{}\n\n=== DIFFS ===\n\n{}",
        TRIAGE_PROMPT,
        pr_title,
        relevant_files.len(),
        file_info,
        diffs
    )
}

pub fn build_summary_prompt(
    pr_title: &str,
    relevant_files: &[&FileClassification],
) -> String {
    build_file_context_prompt(SUMMARY_PROMPT, pr_title, relevant_files)
}

pub fn build_grouping_prompt(
    pr_title: &str,
    relevant_files: &[&FileClassification],
) -> String {
    build_file_context_prompt(GROUPING_PROMPT, pr_title, relevant_files)
}

fn build_file_context_prompt(
    system_prompt: &str,
    pr_title: &str,
    relevant_files: &[&FileClassification],
) -> String {
    let mut file_info = String::new();
    for f in relevant_files {
        file_info.push_str(&format!(
            "- {} [{}] [{}] — {}\n",
            f.path, f.category, f.risk_level, f.reason
        ));
    }

    format!(
        "{}\n\n---\n\nPR Title: {}\n\nRelevant files ({} total):\n\n{}",
        system_prompt,
        pr_title,
        relevant_files.len(),
        file_info
    )
}

/// Truncate `s` to at most `max` chars (char-boundary safe — unlike byte
/// slicing, which can panic mid-character on multibyte input) if it exceeds
/// the budget. Returns `None` when no truncation is needed. Mirrors the
/// equivalent helper in chat.rs.
fn truncate_chars(s: &str, max: usize) -> Option<String> {
    if s.chars().count() <= max {
        None
    } else {
        Some(s.chars().take(max).collect())
    }
}

pub fn build_classification_prompt(
    pr_title: &str,
    file_list: &[String],
    diff_content: &str,
) -> String {
    let files_str = file_list.join("\n");

    // Truncate diff to ~30000 chars for the AI prompt
    let truncated_diff = match truncate_chars(diff_content, 30000) {
        Some(t) => format!("{}\n\n... (diff truncated for brevity)", t),
        None => diff_content.to_string(),
    };

    format!(
        "{}\n\n---\n\nPR Title: {}\n\nFiles changed in this PR:\n\n{}\n\n=== DIFF CONTENT (for context) ===\n\n{}",
        CLASSIFICATION_PROMPT, pr_title, files_str, truncated_diff
    )
}

/// A previously reviewed highlight, fed back into the prompt so re-analysis
/// doesn't re-flag a concern that was already triaged. `state` is
/// "fixed" | "intentional" | "noise" | "" (plain dismiss, no resolution recorded).
pub struct PriorNote {
    pub path: String,
    pub comment: String,
    pub state: String,
    pub reason: String,
}

pub fn build_highlight_prompt(
    pr_title: &str,
    pr_body: &str,
    per_file_diffs: &[(String, String)], // (path, diff)
    prior_notes: &[PriorNote],
) -> String {
    let mut context = String::new();
    for (path, diff) in per_file_diffs {
        context.push_str(&format!("=== FILE: {} ===\n", path));
        // Truncate per-file diff to 5000 chars
        match truncate_chars(diff, 5000) {
            Some(t) => {
                context.push_str(&t);
                context.push_str("\n... (truncated)\n");
            }
            None => context.push_str(diff),
        }
        context.push_str("\n\n");
    }

    let body_section = if pr_body.trim().is_empty() {
        String::new()
    } else {
        let truncated_body = match truncate_chars(pr_body, 2000) {
            Some(t) => format!("{}\n... (truncated)", t),
            None => pr_body.to_string(),
        };
        format!("\nPR Description:\n{}\n", truncated_body)
    };

    let prior_section = if prior_notes.is_empty() {
        String::new()
    } else {
        let mut s =
            String::from("\n=== PREVIOUSLY REVIEWED NOTES (do not re-flag; see instructions) ===\n");
        for note in prior_notes {
            let label = match note.state.as_str() {
                "" | "noise" => "dismissed",
                other => other,
            };
            s.push_str(&format!("- [{}] {}: {}", label, note.path, note.comment));
            if !note.reason.is_empty() {
                s.push_str(&format!(" — reviewer: {}", note.reason));
            }
            s.push('\n');
        }
        s
    };

    format!(
        "{}\n\n---\n\nPR Title: {}\n{}{}\n{}",
        HIGHLIGHT_PROMPT, pr_title, body_section, prior_section, context
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_prompt_includes_pr_body_section() {
        let prompt = build_highlight_prompt(
            "Add retry logic",
            "This PR adds exponential backoff to the fetch client.",
            &[("client.rs".to_string(), "some diff".to_string())],
            &[],
        );
        assert!(prompt.contains("PR Description:"));
        assert!(prompt.contains("This PR adds exponential backoff to the fetch client."));
    }

    #[test]
    fn highlight_prompt_omits_body_section_when_empty() {
        let prompt = build_highlight_prompt(
            "Add retry logic",
            "",
            &[("client.rs".to_string(), "some diff".to_string())],
            &[],
        );
        assert!(!prompt.contains("PR Description:"));
    }

    #[test]
    fn highlight_prompt_includes_prior_notes_section() {
        let prior_notes = vec![
            PriorNote {
                path: "a.rs".to_string(),
                comment: "Null check removed".to_string(),
                state: "fixed".to_string(),
                reason: "restored in follow-up commit".to_string(),
            },
            PriorNote {
                path: "b.rs".to_string(),
                comment: "Config default changed".to_string(),
                state: "intentional".to_string(),
                reason: String::new(),
            },
            PriorNote {
                path: "c.rs".to_string(),
                comment: "Unused import".to_string(),
                state: String::new(),
                reason: String::new(),
            },
        ];
        let prompt = build_highlight_prompt("Title", "", &[], &prior_notes);
        assert!(prompt.contains("PREVIOUSLY REVIEWED NOTES"));
        assert!(prompt.contains("- [fixed] a.rs: Null check removed — reviewer: restored in follow-up commit"));
        assert!(prompt.contains("- [intentional] b.rs: Config default changed"));
        assert!(prompt.contains("- [dismissed] c.rs: Unused import"));
    }

    #[test]
    fn highlight_prompt_omits_prior_section_when_empty() {
        let prompt = build_highlight_prompt("Title", "", &[], &[]);
        assert!(!prompt.contains("PREVIOUSLY REVIEWED NOTES"));
    }

    #[test]
    fn highlight_prompt_per_file_truncation_is_char_safe_on_multibyte() {
        // A >5000-char diff made entirely of a 3-byte multibyte char would panic
        // under `&diff[..5000]` byte slicing (it lands mid-character). Chars,
        // not bytes, must be counted and sliced.
        let diff: String = std::iter::repeat('★').take(6000).collect();
        let prompt = build_highlight_prompt(
            "Title",
            "",
            &[("weird.rs".to_string(), diff)],
            &[],
        );
        assert!(prompt.contains("... (truncated)"));
    }

    #[test]
    fn classification_prompt_truncation_is_char_safe_on_multibyte() {
        let diff: String = std::iter::repeat('★').take(31000).collect();
        let prompt = build_classification_prompt("Title", &["a.rs".to_string()], &diff);
        assert!(prompt.contains("diff truncated for brevity"));
    }

    #[test]
    fn pr_body_truncation_is_char_safe_on_multibyte() {
        let body: String = std::iter::repeat('★').take(2500).collect();
        let prompt = build_highlight_prompt(
            "Title",
            &body,
            &[],
            &[],
        );
        assert!(prompt.contains("PR Description:"));
        assert!(prompt.contains("... (truncated)"));
    }

    #[test]
    fn is_test_path_matches_known_test_conventions() {
        assert!(is_test_path("src/foo.test.ts"));
        assert!(is_test_path("tests/bar.rs"));
        assert!(is_test_path("__tests__/x.js"));
        assert!(is_test_path("e2e/y.ts"));
        assert!(is_test_path("a/b.spec.tsx"));
        assert!(is_test_path("playwright/checkout.ts"));
        assert!(is_test_path("cypress/e2e_setup.ts"));
        assert!(is_test_path("pact/consumer.ts"));
    }

    #[test]
    fn is_test_path_does_not_false_positive_on_substrings() {
        assert!(!is_test_path("src/testing_utils.rs"));
        assert!(!is_test_path("src/attest.rs"));
        assert!(!is_test_path("contest/x.ts"));
    }
}
