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
fn truncate(s: &str, max: usize) -> String {
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
- You are reviewing, not writing the PR — don't propose to make edits; explain, assess risk, and surface what's worth a closer look."#;

/// Documents the `marrow-action` fenced-block protocol: a way for the model to
/// drive the app's view (open a file, flip a filter, hop to a commit) instead
/// of just describing it. The frontend (RichText.tsx) recognizes a fenced
/// code block whose language is exactly `marrow-action`, executes the JSON
/// object once its fence completes, and renders it as a chip rather than code.
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

Usage rules:
- Act only when the user asks to see/navigate/show something, or it clearly helps them get there — a plain answer needs no actions.
- At most a few actions per reply.
- Put the action block AFTER the sentence explaining what you're doing, never instead of it.
- Never invent a path — only use files listed under FILES IN SCOPE.
- One JSON object per fence; use separate fences for multiple actions."#;

/// Assemble the system prompt: the assistant instructions, the UI-actions
/// protocol, then the PR title, AI summary, and the in-scope file diffs
/// (budget-bounded).
pub fn build_chat_system(ctx: &ChatContext) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str(CHAT_SYSTEM_PREAMBLE);
    out.push_str("\n\n--- UI ACTIONS ---\n\n");
    out.push_str(CHAT_UI_ACTIONS);
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
        let sys = build_chat_system(&ctx_with("@@ -1 +1 @@\n-old\n+new\n", None));
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
    fn system_includes_title_summary_and_diff() {
        let sys = build_chat_system(&ctx_with("@@ -1 +1 @@\n-old\n+new\n", None));
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
        let sys = build_chat_system(&ctx);
        // Preamble + UI-actions guide + context, but bounded well under the
        // sum of all inputs. The fixed-overhead margin covers the constant
        // instructional text (preamble, UI actions protocol, section labels).
        assert!(sys.chars().count() < TOTAL_CONTEXT_BUDGET + 2500);
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
        let sys = build_chat_system(&ctx);
        assert!(sys.contains("AI review notes"));
        assert!(sys.contains("L287-318"));
        assert!(sys.contains("SSE parser splits"));
    }
}
