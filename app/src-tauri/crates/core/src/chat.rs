use serde::Deserialize;

/// An AI review note (highlight) on a file, surfaced to the chat so questions
/// like "is the warning on L287-318 legitimate?" resolve against it.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatHighlight {
    pub start_line: u64,
    pub end_line: u64,
    pub severity: String,
    pub comment: String,
}

/// One file's diff/content supplied as grounding for the chat. `head_content` is
/// the full post-change file (present only when the frontend chooses to include
/// it); `unified_diff` is always present. `highlights` are the AI notes the
/// reviewer sees inline on this file.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatFileContext {
    pub path: String,
    pub unified_diff: String,
    #[serde(default)]
    pub head_content: Option<String>,
    #[serde(default)]
    pub highlights: Vec<ChatHighlight>,
}

/// The grounding context for a chat request: PR metadata plus the file(s) in
/// scope (the selected file, or all relevant files when the user opts into
/// whole-PR scope on the frontend).
#[derive(Debug, Clone, Deserialize)]
pub struct ChatContext {
    pub pr_title: String,
    #[serde(default)]
    pub summary: String,
    pub files: Vec<ChatFileContext>,
}

/// Per-file diff budget (chars). Matches the highlight pipeline's per-file cap.
const PER_FILE_DIFF_BUDGET: usize = 5000;
/// Per-file full-content budget (chars) when head content is included.
const PER_FILE_CONTENT_BUDGET: usize = 8000;
/// Overall ceiling on the assembled file context (chars).
const TOTAL_CONTEXT_BUDGET: usize = 30000;

/// Truncate `s` to at most `max` chars on a char boundary, appending a marker
/// when truncated. Safe for multibyte input (unlike slicing by byte index).
/// `pub(crate)` so `chat_agent` can reuse it for tool-result char caps.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("\n... (truncated)\n");
    out
}

/// The review-assistant instructions that precede the grounding context.
const CHAT_SYSTEM_PREAMBLE: &str = r#"You are a code review assistant embedded in a pull-request review tool. A reviewer is reading a diff and asking you questions about it. Answer their questions grounded in the PR context provided below.

Guidelines:
- Answer from the provided diff and file context. If the context doesn't contain enough to answer confidently (e.g. a caller outside the changed files), say so plainly rather than guessing.
- Be concise and direct. Reviewers want fast, high-signal answers — a few sentences, not an essay.
- When useful, reference specific files and line numbers from the diff.
- Use Markdown. Put code in fenced code blocks.
- You are reviewing, not writing the PR — don't propose to make edits; explain, assess risk, and surface what's worth a closer look. The one exception: when the user asks you to comment on something, drafting a PR comment for them (via the draft_comment / draft_pr_comment actions) is allowed and encouraged — the user reviews and posts it themselves."#;

/// Documents the `marrow-action` fenced-block protocol: a way for the model to
/// drive the app's view (open a file, flip a filter, hop to a commit) instead
/// of just describing it. The frontend (RichText.tsx) recognizes a fenced
/// code block whose language is exactly `marrow-action`, executes the JSON
/// object once its fence completes, and renders it as a chip rather than code.
// The action protocol is TRIPLICATED by hand: this prompt section, the
// `ChatAction` union in app/src/types.ts, and the `isChatAction` validator in
// app/src/components/RichText.tsx. Adding or changing an action means editing
// all three, or the model will emit blocks the chips silently reject. The
// `marrow-card` protocol just below is triplicated the same way, across its
// own three sites — see the comment on CHAT_ANSWER_CARDS.
const CHAT_UI_ACTIONS: &str = r#"You can control the review app by emitting a fenced code block with the language `marrow-action` containing exactly one JSON object. The app executes it once and renders it as a chip instead of raw JSON — never describe the JSON in prose, just emit the block.

Available actions (the complete list — never invent others):
- {"action":"open_file","path":"<repo path>","line":<optional number>} — open a changed file, optionally at a head line
- {"action":"open_overview"} — back to the PR overview
- {"action":"next_file"} — the next file in review order
- {"action":"prev_file"} — the previous file in review order
- {"action":"open_commit","sha":"<sha or prefix>"} — open a commit in commit scope
- {"action":"set_hunk_filter","filter":"all|high|medium"} — set the hunk significance filter
- {"action":"set_view_mode","mode":"split|unified"} — set the diff layout
- {"action":"show_comments","open":true|false} — open or close the comments panel
- {"action":"draft_comment","path":"<repo path>","line":<number>,"start_line":<optional number>,"body":"<comment text>"} — propose a PR comment draft: the app opens its inline comment composer at that location with `body` prefilled, and the user edits and posts it themselves. `line` is the anchor (last) line on the head side; optional `start_line` (must be < line) makes it a multi-line range. Anchor ONLY to line numbers visible in the provided diffs as changed (+) or context lines within hunks — GitHub rejects comments anchored outside diff hunks. Write `body` as the final comment in the reviewer's own voice: concise and actionable, no AI-severity prefixes, no meta-commentary.
- {"action":"draft_pr_comment","body":"<comment text>"} — propose a PR-level (conversation) comment draft, not anchored to any file or line: the app opens the Comments panel with its compose box prefilled, and the user edits and posts it themselves. Use it for overall feedback on the PR. Same `body` rules as draft_comment: final comment in the reviewer's own voice, no AI prefixes.

Usage rules:
- Act only when the user asks to see/navigate/show something, or it clearly helps them get there — a plain answer needs no actions.
- At most a few actions per reply.
- Put the action block AFTER the sentence explaining what you're doing, never instead of it.
- Never invent a path — only use files listed under FILES IN SCOPE.
- One JSON object per fence; use separate fences for multiple actions.
- draft_comment / draft_pr_comment only when the user asked for a comment (or explicitly confirm intent first), and at most 2 drafts per reply shared across both. Neither chip ever runs automatically — the user clicks it to open the composer, so nothing is posted without them."#;

/// Documents the `marrow-card` fenced-block protocol: a way for the model to
/// present a tabular or enumerated answer as a structured, renderable card
/// instead of prose or a Markdown table. The frontend (RichText.tsx, via
/// ChatCards.tsx) recognizes a fenced code block whose language is exactly
/// `marrow-card`, parses its one JSON object, and renders a table or list
/// widget in its place — pure rendering, never executed like an action.
// The card protocol is TRIPLICATED by hand, the same way the action protocol
// above is: this prompt section, the `ChatCard` union in app/src/types.ts,
// and the `isChatCard` validator in app/src/components/ChatCards.tsx. Adding
// or changing a schema means editing all three, or the app will silently
// fall back to rendering a plain code block.
const CHAT_ANSWER_CARDS: &str = r#"When an answer is naturally a table or a list of things, present it as a fenced code block with the language `marrow-card` containing exactly one JSON object. The app renders it as a structured card instead of raw JSON — never describe the JSON in prose, just emit the block.

Two schemas (v1 — the only two):
- Table: {"type":"table","title":"<optional>","columns":["Col A","Col B"],"rows":[["cell","cell"],...]}
- List: {"type":"list","title":"<optional>","items":[{"text":"...","detail":"<optional>","path":"<optional>","line":<optional number>},...]}

A table cell is either a plain string or {"text":"...","path":"<optional>","line":<optional number>} when it should jump to a file location.

Usage rules:
- Use a card when the answer is naturally tabular or an enumeration — files affected by something, a list of locations, a comparison across options.
- One JSON object per fence.
- Put a short sentence BEFORE the card saying what it shows — never repeat the card's contents in prose afterward.
- Only use `path` values for files listed under FILES IN SCOPE — never invent one.
- Keep tables to at most 20 rows and 5 columns.
- A plain conversational answer needs no card — don't force one."#;

/// Documents the `marrow-tool` fenced-block protocol (issue #150): read-only
/// repo tools the model can call mid-answer when the diff context alone can't
/// answer the question. Unlike `marrow-action`/`marrow-card`, execution is
/// backend-only (`chat_agent.rs`) — the frontend (RichText.tsx) only renders
/// the fence as a chip, never runs anything.
// The tool protocol is TRIPLICATED by hand, the same way the action and card
// protocols above are: this prompt section, the `ChatToolCall` union in
// app/src/types.ts, and the `isChatToolCall` validator in
// app/src/components/RichText.tsx. Adding or changing a tool means editing
// all three, or the model will emit calls the executor silently rejects.
const CHAT_REPO_TOOLS: &str = r#"You can read the repository beyond the diff with read-only tools. To use one, emit a fenced code block with the language `marrow-tool` containing exactly one JSON object, then END YOUR REPLY immediately after the closing fence — the tool result arrives in the next user message and you continue your answer from there. The app renders the block as a chip.

Available tools (the complete list — never invent others):
- {"tool":"read_file","path":"<repo path>"} — full contents of any repo file at the PR head commit
- {"tool":"search_code","query":"<terms>"} — search this repository's code (GitHub code search indexes the default branch, so treat results as approximate and confirm with read_file)
- {"tool":"list_dir","path":"<directory path, empty string for repo root>"} — list a directory at the PR head commit

Usage rules:
- Reach for a tool when the provided context can't answer — callers outside the changed files, an unfamiliar helper, "is this used elsewhere?". Prefer a tool call over guessing or saying you can't see the code.
- Write a short sentence saying what you're checking BEFORE the block.
- At most 5 tool calls per question — when they're spent, answer with what you have.
- Exactly one JSON object per fence, nothing after the closing fence.
- Tool results are not carried between questions — re-read if you need them again."#;

/// Assemble the system prompt: the assistant instructions, the UI-actions
/// protocol, the answer-cards protocol, the repo-tools protocol (when
/// `repo_tools` is true), then the PR title, AI summary, and the in-scope
/// file diffs (budget-bounded).
pub fn build_chat_system(ctx: &ChatContext, repo_tools: bool) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str(CHAT_SYSTEM_PREAMBLE);
    out.push_str("\n\n--- UI ACTIONS ---\n\n");
    out.push_str(CHAT_UI_ACTIONS);
    out.push_str("\n\n--- ANSWER CARDS ---\n\n");
    out.push_str(CHAT_ANSWER_CARDS);
    if repo_tools {
        out.push_str("\n\n--- REPO TOOLS ---\n\n");
        out.push_str(CHAT_REPO_TOOLS);
    }
    out.push_str("\n\n--- PR CONTEXT ---\n\n");
    out.push_str(&format!("PR Title: {}\n", ctx.pr_title));
    if !ctx.summary.trim().is_empty() {
        out.push_str(&format!("\nPR Summary:\n{}\n", ctx.summary.trim()));
    }

    out.push_str("\n--- FILES IN SCOPE ---\n");
    let mut remaining = TOTAL_CONTEXT_BUDGET;
    for file in &ctx.files {
        if remaining == 0 {
            out.push_str("\n... (additional files omitted to fit context budget)\n");
            break;
        }
        out.push_str(&format!("\n=== FILE: {} ===\n", file.path));

        // AI review notes the reviewer sees inline — so "the warning on L287-318"
        // resolves to something concrete.
        if !file.highlights.is_empty() {
            out.push_str("AI review notes on this file:\n");
            for h in &file.highlights {
                let lines = if h.start_line == h.end_line {
                    format!("L{}", h.start_line)
                } else {
                    format!("L{}-{}", h.start_line, h.end_line)
                };
                out.push_str(&format!("- [{}] {}: {}\n", h.severity, lines, h.comment));
            }
        }

        let diff = truncate(&file.unified_diff, PER_FILE_DIFF_BUDGET.min(remaining));
        remaining = remaining.saturating_sub(diff.chars().count());
        out.push_str("Diff:\n");
        out.push_str(&diff);
        out.push('\n');

        if let Some(content) = &file.head_content {
            if remaining > 0 && !content.trim().is_empty() {
                let content = truncate(content, PER_FILE_CONTENT_BUDGET.min(remaining));
                remaining = remaining.saturating_sub(content.chars().count());
                out.push_str("\nCurrent file contents (post-change):\n");
                out.push_str(&content);
                out.push('\n');
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(diff: &str, content: Option<&str>) -> ChatContext {
        ChatContext {
            pr_title: "Test PR".to_string(),
            summary: "A summary.".to_string(),
            files: vec![ChatFileContext {
                path: "src/lib.rs".to_string(),
                unified_diff: diff.to_string(),
                head_content: content.map(str::to_string),
                highlights: vec![],
            }],
        }
    }

    #[test]
    fn truncate_is_char_safe_and_marks() {
        // A multibyte char repeated past the limit must not panic and must mark.
        let s = "é".repeat(100);
        let t = truncate(&s, 10);
        assert!(t.contains("(truncated)"));
        assert!(t.chars().count() < s.chars().count() + 5);
    }

    #[test]
    fn short_text_is_untouched() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn system_documents_ui_actions() {
        let sys = build_chat_system(&ctx_with("@@ -1 +1 @@\n-old\n+new\n", None), false);
        assert!(sys.contains("UI ACTIONS"));
        assert!(sys.contains("marrow-action"));
        assert!(sys.contains(r#"{"action":"open_file""#));
        assert!(sys.contains(r#"{"action":"open_overview"}"#));
        assert!(sys.contains(r#"{"action":"open_commit""#));
        assert!(sys.contains(r#"{"action":"set_hunk_filter""#));
        assert!(sys.contains(r#"{"action":"set_view_mode""#));
        assert!(sys.contains(r#"{"action":"show_comments""#));
    }

    #[test]
    fn system_documents_answer_cards() {
        let sys = build_chat_system(&ctx_with("@@ -1 +1 @@\n-old\n+new\n", None), false);
        assert!(sys.contains("ANSWER CARDS"));
        assert!(sys.contains("marrow-card"));
        assert!(sys.contains(r#"{"type":"table""#));
        assert!(sys.contains(r#"{"type":"list""#));
    }

    #[test]
    fn system_documents_repo_tools() {
        let sys = build_chat_system(&ctx_with("@@ -1 +1 @@\n-old\n+new\n", None), true);
        assert!(sys.contains("REPO TOOLS"));
        assert!(sys.contains("marrow-tool"));
        assert!(sys.contains(r#"{"tool":"read_file""#));
        assert!(sys.contains(r#"{"tool":"search_code""#));
        assert!(sys.contains(r#"{"tool":"list_dir""#));
    }

    #[test]
    fn repo_tools_absent_when_disabled() {
        let sys = build_chat_system(&ctx_with("@@ -1 +1 @@\n-old\n+new\n", None), false);
        assert!(!sys.contains("marrow-tool"));
        assert!(!sys.contains("REPO TOOLS"));
    }

    #[test]
    fn system_includes_title_summary_and_diff() {
        let sys = build_chat_system(&ctx_with("@@ -1 +1 @@\n-old\n+new\n", None), false);
        assert!(sys.contains("Test PR"));
        assert!(sys.contains("A summary."));
        assert!(sys.contains("src/lib.rs"));
        assert!(sys.contains("+new"));
    }

    #[test]
    fn total_budget_caps_many_huge_files() {
        let big = "x".repeat(40_000);
        let ctx = ChatContext {
            pr_title: "Big".to_string(),
            summary: String::new(),
            files: (0..10)
                .map(|i| ChatFileContext {
                    path: format!("f{i}.rs"),
                    unified_diff: big.clone(),
                    head_content: Some(big.clone()),
                    highlights: vec![],
                })
                .collect(),
        };
        let sys = build_chat_system(&ctx, true);
        // Preamble + UI-actions guide + answer-cards guide + repo-tools guide +
        // context, but bounded well under the sum of all inputs. The
        // fixed-overhead margin covers the constant instructional text
        // (preamble, UI actions protocol, answer cards protocol, repo tools
        // protocol, section labels) — bumped when the repo-tools protocol
        // (issue #150) and the draft_comment and draft_pr_comment actions
        // (issue #185) were added.
        assert!(sys.chars().count() < TOTAL_CONTEXT_BUDGET + 7000);
    }

    #[test]
    fn highlights_are_rendered_with_line_ranges() {
        let mut ctx = ctx_with("@@ -1 +1 @@\n+x\n", None);
        ctx.files[0].highlights = vec![ChatHighlight {
            start_line: 287,
            end_line: 318,
            severity: "warning".to_string(),
            comment: "SSE parser splits on \\n only".to_string(),
        }];
        let sys = build_chat_system(&ctx, false);
        assert!(sys.contains("AI review notes"));
        assert!(sys.contains("L287-318"));
        assert!(sys.contains("SSE parser splits"));
    }
}
