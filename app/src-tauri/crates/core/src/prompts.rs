use crate::budgets;
use crate::types::{FileClassification, LinkedIssue};

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
- Barrel/index files that only re-export: Python __init__.py files that are empty or contain only imports, re-exports (from .foo import Bar), or __all__ declarations; JS/TS index files that only re-export
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
- An __init__.py (or index barrel) that defines actual classes, functions, or substantive logic beyond re-exports IS relevant — the barrel exclusion is only for pure re-export/empty files, and those are NOT_RELEVANT with "low" risk, never medium
- Import-for-side-effect modules are NOT barrels: an __init__.py whose imports exist to trigger registration (bare `import pkg.plugins` style imports whose names are never re-exported or listed in __all__, or imports commented as registering handlers/routes) carries real behavior — classify it RELEVANT with "low" risk

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

Step 1: Extract the explicit requirements or acceptance criteria. When a "USER-PROVIDED REQUIREMENTS" section is present below, extract from IT — it is authoritative — and use the PR description only as supporting context. Otherwise, when a "LINKED ISSUES" section is present, extract from the linked issues and the PR title/description together — issues typically state the acceptance criteria while the description states intent. Otherwise extract from the PR title/description. Never invent or infer a requirement the author didn't state. Skip boilerplate (checklists about code style, formatting, unrelated process items). Extract at most 8 requirements; each must be a short, standalone statement of what the PR is supposed to do or guarantee. Quote each requirement's "text" using the source's own wording as closely as possible (verbatim phrases, not a paraphrase) so re-running this extraction yields identical text. If the source states no real requirements, return {"requirements": [], "orphan_tests": []}.

Step 2: For each requirement, judge it ONLY against the provided test evidence. Evidence comes from up to three sections below: diffs of test files changed in this PR, implementation-file diffs that ADD inline tests (e.g. Rust `#[cfg(test)]` modules), and existing test files unchanged in this PR (shown as full content). In implementation diffs, ONLY the added test code counts as evidence — the implementation changes themselves never make a requirement covered.
- "covered": a shown test genuinely asserts this requirement. The test may come from any provided section: a changed test-file diff, an existing unchanged test file, or the test code inside an implementation diff.
- "partial": a shown test touches this requirement but asserts something weaker than stated — say what's missing in "note".
- "uncovered": no shown test exercises this requirement.
- "untestable": not verifiable by an automated test (e.g. visual polish, subjective wording).
Some diffs or files may end with "... (truncated)". If truncation is what prevents you from confirming coverage, prefer "partial" with a note saying the content was cut — never a confident "uncovered".

For each requirement's "tests", list ONLY paths from the files you were given, citing the file's path regardless of which section it appeared in — never invent a path.

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

/// Detect inline tests ADDED by a diff to an implementation file — Rust
/// `#[cfg(test)]` modules live inside the file they test, invisible to the
/// path-based `is_test_path`. Only added lines count: a marker on a context
/// or removed line is pre-existing (or deleted) test code, not evidence this
/// PR added tests.
pub fn has_inline_test_markers(unified_diff: &str) -> bool {
    unified_diff.lines().any(|line| {
        line.starts_with('+')
            && !line.starts_with("+++")
            && (line.contains("#[test]")
                || line.contains("#[cfg(test)]")
                || line.contains("mod tests"))
    })
}

/// Slice an implementation diff down to its test-relevant tail: keep hunks
/// from the first marker-containing hunk onward (Rust convention puts test
/// modules at the bottom of a file, so once a hunk adds test code, later
/// hunks are test territory too). Budgets then spend on the tests the file
/// was included for, not implementation preamble — a 19k-char impl diff was
/// still truncating away its tests at a 10k cap (issue #191 live-test).
/// Falls back to the whole diff when no hunk boundary or marker is found.
pub fn extract_test_hunks(unified_diff: &str) -> String {
    let mut hunks: Vec<Vec<&str>> = Vec::new();
    for line in unified_diff.lines() {
        if line.starts_with("@@") || hunks.is_empty() {
            hunks.push(Vec::new());
        }
        hunks.last_mut().unwrap().push(line);
    }
    let first_marker = hunks.iter().position(|h| has_inline_test_markers(&h.join("\n")));
    match first_marker {
        Some(i) => hunks[i..]
            .iter()
            .map(|h| h.join("\n"))
            .collect::<Vec<_>>()
            .join("\n"),
        None => unified_diff.to_string(),
    }
}

/// Append one budgeted file section (header + possibly-truncated text) to
/// `out`, drawing from the shared `remaining` pool. Shared by the highlight,
/// triage, and coverage builders. Returns true when the section was cut (or
/// skipped outright because the pool was exhausted) — truncation telemetry
/// for the manifest's `analysis_truncated` flag.
fn append_budgeted_file(
    out: &mut String,
    header: &str,
    text: &str,
    remaining: &mut usize,
    per_file_cap: usize,
) -> bool {
    if *remaining == 0 {
        // Leave a stub so the model knows the file exists but went unseen —
        // silently absent files read as "unchanged", which is worse.
        out.push_str(header);
        out.push_str("(omitted — analysis budget exhausted)\n\n");
        return true;
    }
    out.push_str(header);
    let cap = per_file_cap.min(*remaining);
    let truncated = if text.chars().count() > cap {
        let snippet: String = text.chars().take(cap).collect();
        out.push_str(&snippet);
        out.push_str("\n... (truncated)\n");
        *remaining = remaining.saturating_sub(cap);
        true
    } else {
        out.push_str(text);
        *remaining = remaining.saturating_sub(text.chars().count());
        false
    };
    out.push_str("\n\n");
    truncated
}

/// Build the requirements-coverage prompt: PR title/body plus the diffs of
/// changed test files only (the model judges coverage from tests, never from
/// implementation). Budgeted like `build_triage_prompt`.
///
/// `inline_test_diffs` are implementation-file diffs that add inline tests
/// (see `has_inline_test_markers`); they share `test_diffs`' budget pool.
/// `existing_tests` are (path, full content) of unchanged test files related
/// to the PR's changes, under their own smaller budget so full files never
/// crowd out the PR's own test changes.
///
/// `user_requirements` is the reviewer's local override (issue #179 phase 2,
/// see `pr_requirements.rs`) — when present and non-empty it's inserted
/// ahead of the PR description as the authoritative extraction source (see
/// `REQUIREMENTS_COVERAGE_PROMPT`'s source-precedence note), letting a
/// reviewer supply requirements a sparse/missing PR description doesn't
/// state.
///
/// `linked_issues` are the issues this PR closes — an extraction source that
/// sits below user requirements but alongside the PR description (callers
/// pass an empty slice when user requirements exist; see the fetch in
/// `fetch.rs`).
pub fn build_requirements_coverage_prompt(
    pr_title: &str,
    pr_body: &str,
    test_diffs: &[(String, String)],
    inline_test_diffs: &[(String, String)],
    existing_tests: &[(String, String)],
    changed_paths: &[String],
    user_requirements: Option<&str>,
    linked_issues: &[LinkedIssue],
) -> (String, bool) {
    let mut truncated = false;

    let body = match truncate_chars(pr_body, budgets::COVERAGE_BODY) {
        Some(t) => {
            truncated = true;
            format!("{}\n... (truncated)", t)
        }
        None => pr_body.to_string(),
    };

    // Same cap as the PR body — user-provided text gets no exemption from
    // the prompt budget discipline.
    let user_requirements_section = match user_requirements {
        Some(text) if !text.trim().is_empty() => {
            let capped = match truncate_chars(text, budgets::COVERAGE_USER_REQS) {
                Some(t) => {
                    truncated = true;
                    format!("{}\n... (truncated)", t)
                }
                None => text.to_string(),
            };
            format!("=== USER-PROVIDED REQUIREMENTS (authoritative) ===\n{}\n\n", capped)
        }
        _ => String::new(),
    };

    // Linked-issue bodies get their own budget pool (issues can be long, and
    // acceptance criteria usually sit near the top).
    let mut linked_issues_section = String::new();
    if !linked_issues.is_empty() {
        linked_issues_section
            .push_str("=== LINKED ISSUES (acceptance criteria often live here) ===\n");
        let mut issue_remaining = budgets::COVERAGE_ISSUE_TOTAL;
        for issue in linked_issues {
            truncated |= append_budgeted_file(
                &mut linked_issues_section,
                &format!("--- Issue #{}: {} ---\n", issue.number, issue.title),
                &issue.body,
                &mut issue_remaining,
                budgets::COVERAGE_ISSUE_PER_ISSUE,
            );
        }
    }

    let mut diffs = String::new();
    let mut remaining = budgets::COVERAGE_TOTAL;
    if test_diffs.is_empty() && inline_test_diffs.is_empty() {
        diffs.push_str("(no test files changed in this PR)\n");
    }
    for (path, diff) in test_diffs {
        truncated |= append_budgeted_file(
            &mut diffs,
            &format!("=== TEST FILE: {} ===\n", path),
            diff,
            &mut remaining,
            budgets::COVERAGE_PER_FILE,
        );
    }

    // Same pool as test_diffs: inline tests are the same kind of evidence.
    let mut inline_diffs = String::new();
    for (path, diff) in inline_test_diffs {
        truncated |= append_budgeted_file(
            &mut inline_diffs,
            &format!("=== IMPLEMENTATION FILE: {} ===\n", path),
            diff,
            &mut remaining,
            budgets::COVERAGE_PER_FILE,
        );
    }

    let mut existing = String::new();
    let mut existing_remaining = budgets::COVERAGE_EXISTING_TOTAL;
    for (path, content) in existing_tests {
        truncated |= append_budgeted_file(
            &mut existing,
            &format!("=== EXISTING TEST FILE: {} ===\n", path),
            content,
            &mut existing_remaining,
            budgets::COVERAGE_EXISTING_PER_FILE,
        );
    }

    let mut prompt = format!(
        "{}\n\n---\n\nPR Title: {}\n\n{}{}PR Description:\n{}\n\nChanged files ({} total):\n{}\n\n=== TEST FILE DIFFS ===\n\n{}",
        REQUIREMENTS_COVERAGE_PROMPT,
        pr_title,
        user_requirements_section,
        linked_issues_section,
        body,
        changed_paths.len(),
        changed_paths.join("\n"),
        diffs
    );
    if !inline_test_diffs.is_empty() {
        prompt.push_str(&format!(
            "=== IMPLEMENTATION DIFFS WITH INLINE TESTS ===\n\n{}",
            inline_diffs
        ));
    }
    if !existing_tests.is_empty() {
        prompt.push_str(&format!(
            "=== EXISTING TEST FILES (unchanged in this PR; full content, may be truncated) ===\n\n{}",
            existing
        ));
    }
    (prompt, truncated)
}

/// Build the triage prompt: structured per-file info (path, category, risk,
/// reason) plus compact diff snippets so the model can reason about contracts and
/// dependencies for ordering. Diffs are budgeted tighter than the highlight pass
/// since ordering needs signatures/imports, not full bodies.
pub fn build_triage_prompt(
    pr_title: &str,
    relevant_files: &[&FileClassification],
    per_file_diffs: &[(String, String)],
) -> (String, bool) {
    let mut file_info = String::new();
    for f in relevant_files {
        file_info.push_str(&format!(
            "- {} [{}] [{}] — {}\n",
            f.path, f.category, f.risk_level, f.reason
        ));
    }

    let mut diffs = String::new();
    let mut remaining = budgets::TRIAGE_TOTAL;
    let mut truncated = false;
    for (path, diff) in per_file_diffs {
        truncated |= append_budgeted_file(
            &mut diffs,
            &format!("=== FILE: {} ===\n", path),
            diff,
            &mut remaining,
            budgets::TRIAGE_PER_FILE,
        );
    }

    let prompt = format!(
        "{}\n\n---\n\nPR Title: {}\n\nRelevant files ({} total):\n\n{}\n\n=== DIFFS ===\n\n{}",
        TRIAGE_PROMPT,
        pr_title,
        relevant_files.len(),
        file_info,
        diffs
    );
    (prompt, truncated)
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

/// Summary/grouping prompts carry only one info line per file (no diffs), so
/// they never truncate and don't participate in `analysis_truncated`.
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
) -> (String, bool) {
    let files_str = file_list.join("\n");

    let (truncated_diff, truncated) = match truncate_chars(diff_content, budgets::CLASSIFICATION_DIFF) {
        Some(t) => (format!("{}\n\n... (diff truncated for brevity)", t), true),
        None => (diff_content.to_string(), false),
    };

    let prompt = format!(
        "{}\n\n---\n\nPR Title: {}\n\nFiles changed in this PR:\n\n{}\n\n=== DIFF CONTENT (for context) ===\n\n{}",
        CLASSIFICATION_PROMPT, pr_title, files_str, truncated_diff
    );
    (prompt, truncated)
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
) -> (String, bool) {
    let mut truncated = false;

    // Per-file cap plus a shared pool across all files — one giant file can't
    // starve the rest, and many large files can't blow the context window.
    let mut context = String::new();
    let mut remaining = budgets::HIGHLIGHT_TOTAL;
    for (path, diff) in per_file_diffs {
        truncated |= append_budgeted_file(
            &mut context,
            &format!("=== FILE: {} ===\n", path),
            diff,
            &mut remaining,
            budgets::HIGHLIGHT_PER_FILE,
        );
    }

    let body_section = if pr_body.trim().is_empty() {
        String::new()
    } else {
        let truncated_body = match truncate_chars(pr_body, budgets::HIGHLIGHT_BODY) {
            Some(t) => {
                truncated = true;
                format!("{}\n... (truncated)", t)
            }
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

    let prompt = format!(
        "{}\n\n---\n\nPR Title: {}\n{}{}\n{}",
        HIGHLIGHT_PROMPT, pr_title, body_section, prior_section, context
    );
    (prompt, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_prompt_includes_pr_body_section() {
        let (prompt, _) = build_highlight_prompt(
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
        let (prompt, _) = build_highlight_prompt(
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
        let (prompt, _) = build_highlight_prompt("Title", "", &[], &prior_notes);
        assert!(prompt.contains("PREVIOUSLY REVIEWED NOTES"));
        assert!(prompt.contains("- [fixed] a.rs: Null check removed — reviewer: restored in follow-up commit"));
        assert!(prompt.contains("- [intentional] b.rs: Config default changed"));
        assert!(prompt.contains("- [dismissed] c.rs: Unused import"));
    }

    #[test]
    fn highlight_prompt_omits_prior_section_when_empty() {
        let (prompt, _) = build_highlight_prompt("Title", "", &[], &[]);
        assert!(!prompt.contains("PREVIOUSLY REVIEWED NOTES"));
    }

    #[test]
    fn highlight_prompt_per_file_truncation_is_char_safe_on_multibyte() {
        // An over-budget diff made entirely of a 3-byte multibyte char would
        // panic under byte slicing (it lands mid-character). Chars, not bytes,
        // must be counted and sliced.
        let diff: String = std::iter::repeat('★').take(budgets::HIGHLIGHT_PER_FILE + 1000).collect();
        let (prompt, truncated) = build_highlight_prompt(
            "Title",
            "",
            &[("weird.rs".to_string(), diff)],
            &[],
        );
        assert!(prompt.contains("... (truncated)"));
        assert!(truncated);
    }

    #[test]
    fn highlight_prompt_reports_no_truncation_for_fitting_input() {
        // (No marker assertion here: HIGHLIGHT_PROMPT's own instructions
        // mention the "... (truncated)" marker, so the flag is the signal.)
        let (_, truncated) = build_highlight_prompt(
            "Title",
            "short body",
            &[("a.rs".to_string(), "small diff".to_string())],
            &[],
        );
        assert!(!truncated);
    }

    #[test]
    fn highlight_prompt_total_pool_caps_many_large_files() {
        // Each file fits its per-file cap, but together they blow the shared
        // pool — later files must be cut/omitted and the flag must report it.
        let per_file = budgets::HIGHLIGHT_PER_FILE - 5_000;
        let n_files = budgets::HIGHLIGHT_TOTAL / per_file + 3;
        let files: Vec<(String, String)> = (0..n_files)
            .map(|i| (format!("f{i}.rs"), "d".repeat(per_file)))
            .collect();
        let (prompt, truncated) = build_highlight_prompt("Title", "", &files, &[]);
        assert!(truncated);
        // Pool + instructions + headers, but nowhere near the raw input sum.
        assert!(prompt.chars().count() < budgets::HIGHLIGHT_TOTAL + 20_000);
    }

    #[test]
    fn classification_prompt_truncation_is_char_safe_on_multibyte() {
        let diff: String = std::iter::repeat('★').take(budgets::CLASSIFICATION_DIFF + 1000).collect();
        let (prompt, truncated) = build_classification_prompt("Title", &["a.rs".to_string()], &diff);
        assert!(prompt.contains("diff truncated for brevity"));
        assert!(truncated);
    }

    #[test]
    fn classification_prompt_reports_no_truncation_for_fitting_diff() {
        let (prompt, truncated) =
            build_classification_prompt("Title", &["a.rs".to_string()], "tiny diff");
        assert!(!prompt.contains("diff truncated for brevity"));
        assert!(!truncated);
    }

    #[test]
    fn pr_body_truncation_is_char_safe_on_multibyte() {
        let body: String = std::iter::repeat('★').take(budgets::HIGHLIGHT_BODY + 500).collect();
        let (prompt, truncated) = build_highlight_prompt(
            "Title",
            &body,
            &[],
            &[],
        );
        assert!(prompt.contains("PR Description:"));
        assert!(prompt.contains("... (truncated)"));
        assert!(truncated);
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
    fn requirements_coverage_prompt_includes_user_requirements_when_provided() {
        let (prompt, _) = build_requirements_coverage_prompt(
            "Title",
            "PR body",
            &[],
            &[],
            &[],
            &["a.rs".to_string()],
            Some("The endpoint must return 404 for unknown ids."),
            &[],
        );
        assert!(prompt.contains("=== USER-PROVIDED REQUIREMENTS (authoritative) ==="));
        assert!(prompt.contains("The endpoint must return 404 for unknown ids."));
    }

    #[test]
    fn requirements_coverage_prompt_omits_user_requirements_when_none() {
        let (prompt, _) = build_requirements_coverage_prompt(
            "Title",
            "PR body",
            &[],
            &[],
            &[],
            &["a.rs".to_string()],
            None,
            &[],
        );
        assert!(!prompt.contains("=== USER-PROVIDED REQUIREMENTS (authoritative) ==="));
    }

    fn linked_issue(number: u64, title: &str, body: &str) -> LinkedIssue {
        LinkedIssue { number, title: title.to_string(), body: body.to_string() }
    }

    #[test]
    fn requirements_coverage_prompt_includes_linked_issues_section() {
        let (prompt, _) = build_requirements_coverage_prompt(
            "Title",
            "PR body",
            &[],
            &[],
            &[],
            &["a.rs".to_string()],
            None,
            &[
                linked_issue(12, "Support dark mode", "The app must honor the OS theme."),
                linked_issue(34, "Fix crash", "Opening an empty file must not crash."),
            ],
        );
        assert!(prompt.contains("=== LINKED ISSUES (acceptance criteria often live here) ==="));
        assert!(prompt.contains("--- Issue #12: Support dark mode ---"));
        assert!(prompt.contains("The app must honor the OS theme."));
        assert!(prompt.contains("--- Issue #34: Fix crash ---"));
        assert!(prompt.contains("Opening an empty file must not crash."));
        // The section sits between the (absent) user requirements and the body.
        let section = prompt.find("=== LINKED ISSUES").unwrap();
        let description = prompt.find("PR Description:").unwrap();
        assert!(section < description);
        // Step 1 tells the model how to weigh the section.
        assert!(REQUIREMENTS_COVERAGE_PROMPT
            .contains(r#"when a "LINKED ISSUES" section is present, extract from the linked issues and the PR title/description together"#));
    }

    #[test]
    fn requirements_coverage_prompt_omits_linked_issues_when_empty() {
        let (prompt, _) = build_requirements_coverage_prompt(
            "Title",
            "PR body",
            &[],
            &[],
            &[],
            &["a.rs".to_string()],
            None,
            &[],
        );
        assert!(!prompt.contains("=== LINKED ISSUES"));
    }

    #[test]
    fn requirements_coverage_prompt_linked_issues_respect_per_issue_budget() {
        let (prompt, truncated) = build_requirements_coverage_prompt(
            "Title",
            "PR body",
            &[],
            &[],
            &[],
            &["a.rs".to_string()],
            None,
            &[linked_issue(1, "Big", &"y".repeat(budgets::COVERAGE_ISSUE_PER_ISSUE + 500))],
        );
        // Exceeds the per-issue cap — cut at the cap.
        assert!(prompt.contains("--- Issue #1: Big ---"));
        assert!(prompt.contains("... (truncated)"));
        assert!(prompt.contains(&"y".repeat(budgets::COVERAGE_ISSUE_PER_ISSUE)));
        assert!(!prompt.contains(&"y".repeat(budgets::COVERAGE_ISSUE_PER_ISSUE + 1)));
        assert!(truncated);
    }

    #[test]
    fn requirements_coverage_prompt_linked_issues_respect_total_budget() {
        // Six 4500-char issues against the 20_000-char pool: four full, the
        // fifth truncated to the 2000 remaining, the sixth reduced to an
        // omission stub.
        let issues: Vec<LinkedIssue> =
            (1..=6).map(|n| linked_issue(n, "Issue", &"z".repeat(4500))).collect();
        let (prompt, truncated) = build_requirements_coverage_prompt(
            "Title",
            "PR body",
            &[],
            &[],
            &[],
            &["a.rs".to_string()],
            None,
            &issues,
        );
        assert!(prompt.contains("--- Issue #4: Issue ---"));
        assert!(prompt.contains("--- Issue #5: Issue ---"));
        assert!(prompt.contains("... (truncated)"));
        assert!(prompt.contains(&"z".repeat(2000)));
        assert!(prompt.contains("--- Issue #6: Issue ---"));
        assert!(prompt.contains("(omitted — analysis budget exhausted)"));
        // No run longer than one full issue survives the caps.
        assert!(!prompt.contains(&"z".repeat(4501)));
        assert!(truncated);
    }

    #[test]
    fn has_inline_test_markers_detects_each_marker_on_added_lines() {
        assert!(has_inline_test_markers("@@ -1 +1 @@\n+#[test]\n+fn works() {}\n"));
        assert!(has_inline_test_markers("@@ -1 +1 @@\n+#[cfg(test)]\n"));
        assert!(has_inline_test_markers("@@ -1 +1 @@\n+mod tests {\n"));
    }

    #[test]
    fn has_inline_test_markers_ignores_context_removed_and_header_lines() {
        // Markers on context or removed lines are pre-existing/deleted tests.
        assert!(!has_inline_test_markers("@@ -1 +1 @@\n #[cfg(test)]\n mod tests {\n"));
        assert!(!has_inline_test_markers("@@ -1 +1 @@\n-#[test]\n-mod tests {\n"));
        // "+++ b/..." file headers start with '+' but aren't added lines.
        assert!(!has_inline_test_markers("--- a/mod tests.rs\n+++ b/#[cfg(test)].rs\n"));
    }

    #[test]
    fn extract_test_hunks_keeps_from_first_marker_hunk_onward() {
        let diff = "@@ -1,3 +1,4 @@\n fn real() {}\n+fn added() {}\n@@ -20,2 +21,4 @@\n+#[cfg(test)]\n+mod tests {\n@@ -30,1 +33,2 @@\n+    #[test]\n+    fn t() {}";
        let sliced = extract_test_hunks(diff);
        assert!(!sliced.contains("fn added()"));
        assert!(sliced.contains("#[cfg(test)]"));
        assert!(sliced.contains("fn t() {}"));
    }

    #[test]
    fn extract_test_hunks_whole_diff_when_marker_in_first_hunk() {
        let diff = "@@ -1,1 +1,2 @@\n+#[test]\n+fn t() {}";
        assert_eq!(extract_test_hunks(diff), diff);
    }

    #[test]
    fn extract_test_hunks_falls_back_when_no_marker_hunks() {
        let diff = "no hunk headers at all";
        assert_eq!(extract_test_hunks(diff), diff);
    }

    #[test]
    fn has_inline_test_markers_negative_on_unrelated_diff() {
        assert!(!has_inline_test_markers("@@ -1 +1 @@\n-old\n+new\n+fn helper() {}\n"));
    }

    #[test]
    fn requirements_coverage_prompt_includes_inline_and_existing_sections() {
        let (prompt, _) = build_requirements_coverage_prompt(
            "Title",
            "PR body",
            &[("tests/a.rs".to_string(), "+assert!(a());".to_string())],
            &[("src/impl.rs".to_string(), "+#[cfg(test)]\n+mod tests {}".to_string())],
            &[("tests/old.rs".to_string(), "fn existing() {}".to_string())],
            &["src/impl.rs".to_string()],
            None,
            &[],
        );
        assert!(prompt.contains("=== TEST FILE: tests/a.rs ==="));
        assert!(prompt.contains("=== IMPLEMENTATION DIFFS WITH INLINE TESTS ==="));
        assert!(prompt.contains("=== IMPLEMENTATION FILE: src/impl.rs ==="));
        assert!(prompt.contains(
            "=== EXISTING TEST FILES (unchanged in this PR; full content, may be truncated) ==="
        ));
        assert!(prompt.contains("=== EXISTING TEST FILE: tests/old.rs ==="));
    }

    #[test]
    fn requirements_coverage_prompt_placeholder_only_when_no_changed_test_evidence() {
        // Inline test diffs alone suppress the "(no test files changed)" line.
        let (prompt, _) = build_requirements_coverage_prompt(
            "Title",
            "PR body",
            &[],
            &[("src/impl.rs".to_string(), "+#[test]".to_string())],
            &[],
            &["src/impl.rs".to_string()],
            None,
            &[],
        );
        assert!(!prompt.contains("(no test files changed in this PR)"));

        // Existing unchanged tests do NOT — they aren't changed in this PR.
        let (prompt, _) = build_requirements_coverage_prompt(
            "Title",
            "PR body",
            &[],
            &[],
            &[("tests/old.rs".to_string(), "fn existing() {}".to_string())],
            &["src/impl.rs".to_string()],
            None,
            &[],
        );
        assert!(prompt.contains("(no test files changed in this PR)"));
        assert!(!prompt.contains("=== IMPLEMENTATION DIFFS WITH INLINE TESTS ==="));
    }

    #[test]
    fn requirements_coverage_prompt_existing_tests_have_own_budget() {
        let big = "y".repeat(budgets::COVERAGE_EXISTING_PER_FILE + 500);
        let (prompt, truncated) = build_requirements_coverage_prompt(
            "Title",
            "PR body",
            &[],
            &[],
            &[("tests/big.rs".to_string(), big)],
            &["src/impl.rs".to_string()],
            None,
            &[],
        );
        // Exceeds the per-file cap for existing test files — cut at the cap.
        assert!(prompt.contains("=== EXISTING TEST FILE: tests/big.rs ==="));
        assert!(prompt.contains("... (truncated)"));
        assert!(!prompt.contains(&"y".repeat(budgets::COVERAGE_EXISTING_PER_FILE + 1)));
        assert!(prompt.contains(&"y".repeat(budgets::COVERAGE_EXISTING_PER_FILE)));
        assert!(truncated);
    }

    #[test]
    fn requirements_coverage_prompt_reports_no_truncation_for_fitting_input() {
        let (_, truncated) = build_requirements_coverage_prompt(
            "Title",
            "PR body",
            &[("tests/a.rs".to_string(), "+assert!(a());".to_string())],
            &[],
            &[("tests/old.rs".to_string(), "fn existing() {}".to_string())],
            &["src/impl.rs".to_string()],
            Some("The endpoint must return 404 for unknown ids."),
            &[],
        );
        assert!(!truncated);
    }

    #[test]
    fn is_test_path_does_not_false_positive_on_substrings() {
        assert!(!is_test_path("src/testing_utils.rs"));
        assert!(!is_test_path("src/attest.rs"));
        assert!(!is_test_path("contest/x.ts"));
    }
}
