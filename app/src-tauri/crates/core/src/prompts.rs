use crate::types::FileClassification;

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

pub const HIGHLIGHT_PROMPT: &str = r#"You are a code review assistant. You are given the diffs of files that have been classified as relevant for review. Your job is to identify specific changes within each file that deserve human attention.

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

For each highlight, provide:
- "path": the file path
- "start_line": the line number in the NEW (head) version of the file where the notable change starts
- "end_line": the line number in the NEW (head) version where it ends
- "severity": one of "critical", "warning", "info"
  - "critical": security risk, data loss risk, auth bypass, breaking API change
  - "warning": behavior change worth verifying, removed safety check, non-obvious side effect
  - "info": worth noting but likely fine — refactored logic, changed defaults, added dependency
- "comment": a concise explanation (under 20 words) of what the reviewer should pay attention to

Respond with ONLY a valid JSON array of these highlight objects. If there are no notable changes, return an empty array [].
Do NOT include any text before or after the JSON array. Just the JSON."#;

pub const SUMMARY_PROMPT: &str = r#"You are a code review assistant. Given a PR title and a list of relevant files with their classifications and AI-generated reasons, write a SHORT orienting summary for a code reviewer.

Keep it tight — at most 3-4 sentences total (one short paragraph, or two at most). Cover only:
1. What this PR does, in one or two sentences.
2. The single most important thing to focus on, or the overall shape (e.g. "mostly a refactor with one behavioral change in X").

Do not enumerate files, restate per-file reasons, or list every risk — a separate triage step already surfaces the specific risks. Be brief; the reviewer wants orientation, not analysis. No JSON, no markdown headers, no bullet points — just one or two short prose paragraphs."#;

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

pub fn build_classification_prompt(
    pr_title: &str,
    file_list: &[String],
    diff_content: &str,
) -> String {
    let files_str = file_list.join("\n");

    // Truncate diff to ~30000 chars for the AI prompt
    let truncated_diff = if diff_content.len() > 30000 {
        format!(
            "{}\n\n... (diff truncated for brevity)",
            &diff_content[..30000]
        )
    } else {
        diff_content.to_string()
    };

    format!(
        "{}\n\n---\n\nPR Title: {}\n\nFiles changed in this PR:\n\n{}\n\n=== DIFF CONTENT (for context) ===\n\n{}",
        CLASSIFICATION_PROMPT, pr_title, files_str, truncated_diff
    )
}

pub fn build_highlight_prompt(
    pr_title: &str,
    per_file_diffs: &[(String, String)], // (path, diff)
) -> String {
    let mut context = String::new();
    for (path, diff) in per_file_diffs {
        context.push_str(&format!("=== FILE: {} ===\n", path));
        // Truncate per-file diff to 5000 chars
        if diff.len() > 5000 {
            context.push_str(&diff[..5000]);
            context.push_str("\n... (truncated)\n");
        } else {
            context.push_str(diff);
        }
        context.push_str("\n\n");
    }

    format!(
        "{}\n\n---\n\nPR Title: {}\n\n{}",
        HIGHLIGHT_PROMPT, pr_title, context
    )
}
