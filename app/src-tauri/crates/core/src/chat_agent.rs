//! The chat's agentic tool-use loop (issue #150): lets the model call
//! read-only repo tools mid-answer via a ```marrow-tool fenced block (see
//! `CHAT_REPO_TOOLS` in chat.rs, the single source of truth for the
//! protocol). Provider-agnostic — it works the same for every `AiBackend`
//! variant because it only relies on `invoke_chat_stream`'s streaming
//! contract (Delta/Status updates, cancel-on-drop), never a provider-specific
//! tool-calling API. Execution is backend-only: the frontend (RichText.tsx)
//! only renders the fence as a chip, unlike `marrow-action`.

use crate::ai::{AiBackend, ChatRole, ChatTurn, StreamUpdate};
use crate::chat::truncate;
use crate::github::GithubClient;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// Which repo (and commit) the tools read from — always the PR under review.
pub struct RepoToolTarget {
    pub owner: String,
    pub repo: String,
    pub head_sha: String,
}

const MAX_TOOL_CALLS: usize = 5;
/// Per-result char cap — matches chat.rs PER_FILE_CONTENT_BUDGET.
const TOOL_RESULT_BUDGET: usize = 8000;
const LIST_DIR_MAX_ENTRIES: usize = 200;

/// One read-only repo tool call the model can request via a ```marrow-tool
/// fence — mirrors the shapes documented in `CHAT_REPO_TOOLS` (chat.rs), the
/// single source of truth for the protocol.
// Kept in sync BY HAND with CHAT_REPO_TOOLS (chat.rs) and the ChatToolCall
// union (app/src/types.ts) — edit all three together.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolCall {
    ReadFile { path: String },
    SearchCode { query: String },
    ListDir {
        #[serde(default)]
        path: String,
    },
}

impl ToolCall {
    /// Transient status shown via `StreamUpdate::Status` while the tool runs.
    fn status_label(&self) -> String {
        match self {
            ToolCall::ReadFile { path } => format!("Reading {path}…"),
            ToolCall::SearchCode { query } => format!("Searching code for \u{201c}{query}\u{201d}…"),
            ToolCall::ListDir { path } => {
                let where_ = if path.is_empty() { "repo root" } else { path.as_str() };
                format!("Listing {where_}…")
            }
        }
    }
}

/// Parse a ```marrow-tool fence body into a [`ToolCall`]. Serde's
/// `deny_unknown_fields` rejects unknown tools/fields; this additionally
/// rejects an empty `path`/`query`, which the documented protocol never
/// emits but a model could still produce.
fn parse_tool_call(json: &str) -> Result<ToolCall, String> {
    let call: ToolCall = serde_json::from_str(json).map_err(|e| e.to_string())?;
    match &call {
        ToolCall::ReadFile { path } if path.trim().is_empty() => {
            Err("read_file requires a non-empty path".to_string())
        }
        ToolCall::SearchCode { query } if query.trim().is_empty() => {
            Err("search_code requires a non-empty query".to_string())
        }
        _ => Ok(call),
    }
}

/// Shared implementation for [`find_tool_fence_end`] / [`find_tool_fence_end_final`].
/// `at_end_closes` controls whether a closer line lacking a trailing newline
/// (i.e. it's the last line in `s`) still counts as closed.
fn find_fence_end_impl(s: &str, at_end_closes: bool) -> Option<usize> {
    let mut in_generic_fence = false;
    let mut in_tool_fence = false;
    let mut offset = 0usize;
    for raw_line in s.split_inclusive('\n') {
        let has_newline = raw_line.ends_with('\n');
        let content = if has_newline { &raw_line[..raw_line.len() - 1] } else { raw_line };
        let trimmed = content.trim_end();
        let line_end = offset + raw_line.len();

        if in_tool_fence {
            if trimmed == "```" {
                if has_newline || at_end_closes {
                    return Some(line_end);
                }
                return None;
            }
            offset = line_end;
            continue;
        }

        if trimmed == "```marrow-tool" && !in_generic_fence {
            in_tool_fence = true;
        } else if trimmed.starts_with("```") {
            // Any other fence-opening/closing line toggles generic fence
            // state, so an opener nested inside an unrelated fence (or a
            // closer for one) never gets mistaken for a marrow-tool opener.
            in_generic_fence = !in_generic_fence;
        }
        offset = line_end;
    }
    None
}

/// Byte index just past the closing fence line of the FIRST complete
/// ```marrow-tool fence in `s`, or None. Streaming-safe: the closing fence
/// line only counts once its trailing newline has arrived (more bytes could
/// still extend the line, e.g. "```x"). Call `find_tool_fence_end_final`
/// instead once the stream has ended.
fn find_tool_fence_end(s: &str) -> Option<usize> {
    find_fence_end_impl(s, false)
}

/// Same, but end-of-input also terminates the closing fence line.
fn find_tool_fence_end_final(s: &str) -> Option<usize> {
    find_fence_end_impl(s, true)
}

/// The JSON body between the opener and closer lines of the first complete
/// ```marrow-tool fence in `visible` (which must already contain one — call
/// after `find_tool_fence_end`/`find_tool_fence_end_final` returns `Some`).
fn extract_tool_json(visible: &str) -> String {
    let mut in_generic_fence = false;
    let mut in_tool_fence = false;
    let mut body: Vec<&str> = Vec::new();
    for raw_line in visible.split_inclusive('\n') {
        let has_newline = raw_line.ends_with('\n');
        let content = if has_newline { &raw_line[..raw_line.len() - 1] } else { raw_line };
        let trimmed = content.trim_end();

        if in_tool_fence {
            if trimmed == "```" {
                break;
            }
            body.push(content);
            continue;
        }

        if trimmed == "```marrow-tool" && !in_generic_fence {
            in_tool_fence = true;
        } else if trimmed.starts_with("```") {
            in_generic_fence = !in_generic_fence;
        }
    }
    body.join("\n")
}

/// Run one tool call against the GitHub API. Never returns `Err` — API
/// failures become a text result the model can read and recover from, same
/// as any other tool output, rather than failing the whole chat turn.
async fn execute_tool(github: &GithubClient, t: &RepoToolTarget, call: &ToolCall) -> String {
    let result = match call {
        ToolCall::ReadFile { path } => {
            match github.get_file_content(&t.owner, &t.repo, path, &t.head_sha).await {
                Ok(content) if content.is_empty() => {
                    format!("File not found (or empty) at PR head: {path}")
                }
                Ok(content) => format!("Contents of {path} at PR head:\n{content}"),
                Err(e) => format!("Tool error: {e}"),
            }
        }
        ToolCall::SearchCode { query } => {
            match github.search_code(&t.owner, &t.repo, query).await {
                Ok((hits, _)) if hits.is_empty() => {
                    format!("No code-search results for \"{query}\" in {}/{}.", t.owner, t.repo)
                }
                Ok((hits, total)) => {
                    let n = hits.len();
                    let mut out = format!(
                        "Code search results for \"{query}\" ({total} total, showing {n}; default branch — confirm against PR head with read_file):\n"
                    );
                    for hit in &hits {
                        out.push_str(&format!("- {}\n", hit.path));
                        for frag in hit.fragments.iter().take(2) {
                            out.push_str(&format!("  {}\n", frag));
                        }
                    }
                    out
                }
                Err(e) => format!("Tool error: {e}"),
            }
        }
        ToolCall::ListDir { path } => {
            match github.list_dir(&t.owner, &t.repo, path, &t.head_sha).await {
                Ok(entries) => {
                    let where_ = if path.is_empty() { "repo root" } else { path.as_str() };
                    let mut out = format!("Contents of {where_} at PR head:\n");
                    let total = entries.len();
                    for entry in entries.iter().take(LIST_DIR_MAX_ENTRIES) {
                        if entry.entry_type == "dir" {
                            out.push_str(&format!("{}  ({})\n", entry.name, entry.entry_type));
                        } else {
                            out.push_str(&format!(
                                "{}  ({}, {} B)\n",
                                entry.name, entry.entry_type, entry.size
                            ));
                        }
                    }
                    if total > LIST_DIR_MAX_ENTRIES {
                        out.push_str(&format!("... ({} more entries)\n", total - LIST_DIR_MAX_ENTRIES));
                    }
                    out
                }
                Err(e) => format!("Tool error: {e}"),
            }
        }
    };
    truncate(&result, TOOL_RESULT_BUDGET)
}

/// Per-segment streaming state shared between the stream callback and the
/// fence-completion race in [`run_chat_agent`].
struct Seg {
    text: String,
    forwarded: usize,
    cut: Option<usize>,
}

/// Drive one chat answer through the tool-use loop: stream text, abort the
/// underlying call the instant a ```marrow-tool fence completes, execute the
/// tool against `github`, feed the result back as a turn, and re-invoke —
/// up to `MAX_TOOL_CALLS` executed calls. The returned transcript (which
/// includes the marrow-tool fences themselves) is what the caller sends as
/// `Done { content }` and persists to history — fences render as chips in
/// the saved transcript, exactly like `marrow-action`/`marrow-card`.
pub async fn run_chat_agent(
    backend: &AiBackend,
    github: &GithubClient,
    target: &RepoToolTarget,
    system: &str,
    mut turns: Vec<ChatTurn>,
    on: &mut (dyn FnMut(StreamUpdate) + Send),
) -> Result<String, String> {
    let mut transcript = String::new();
    let mut calls_used: usize = 0;

    // 5 executed calls + 1 budget-exhausted notice + the forced final segment.
    for _ in 0..(MAX_TOOL_CALLS + 2) {
        let seg = Arc::new(Mutex::new(Seg { text: String::new(), forwarded: 0, cut: None }));
        let notify = Arc::new(Notify::new());

        // The callback owns its own clones of seg/notify (moved in) so it
        // has no lifetime entanglement with the outer loop variables; only
        // `on` is reborrowed, so the borrow ends when `cb` is dropped and
        // `on` becomes usable again for the rest of this iteration.
        let seg_cb = seg.clone();
        let notify_cb = notify.clone();
        let on_reborrow: &mut (dyn FnMut(StreamUpdate) + Send) = &mut *on;
        let mut cb = move |u: StreamUpdate| match u {
            StreamUpdate::Status(s) => on_reborrow(StreamUpdate::Status(s)),
            StreamUpdate::Delta(text) => {
                let mut g = seg_cb.lock().unwrap();
                g.text.push_str(&text);
                if g.cut.is_none() {
                    if let Some(idx) = find_tool_fence_end(&g.text) {
                        g.cut = Some(idx);
                        notify_cb.notify_one();
                    }
                }
                let allowed = g.cut.unwrap_or(g.text.len());
                if allowed > g.forwarded {
                    let piece = g.text[g.forwarded..allowed].to_string();
                    g.forwarded = allowed;
                    drop(g);
                    on_reborrow(StreamUpdate::Delta(piece));
                }
            }
        };

        // Register interest in the fence-completion notification BEFORE
        // racing the stream — Notify stores a permit for the next waiter
        // even if notify_one() fires first, so ordering here is safe either
        // way, but registering early keeps the intent explicit.
        let notified = notify.notified();
        tokio::pin!(notified);
        let stream_result = tokio::select! {
            res = backend.invoke_chat_stream(system, &turns, &mut cb) => Some(res),
            _ = &mut notified => None,
        };
        // Ends `cb`'s reborrow of `on`, freeing it for the status/delta
        // calls below. Dropping the future on the fence-abort path (the
        // `notified` branch winning) closes the stream / kills the `claude`
        // CLI child, same as the existing single-shot cancellation.
        drop(cb);

        // Only propagate an error from the branch that actually completed —
        // when the fence-abort path won the race, `stream_result` is None
        // and there's nothing to propagate.
        if let Some(res) = stream_result {
            res?;
        }

        let (text, cut) = {
            let mut g = seg.lock().unwrap();
            let cut = g.cut.or_else(|| find_tool_fence_end_final(&g.text));
            (std::mem::take(&mut g.text), cut)
        };

        let Some(idx) = cut else {
            transcript.push_str(&text);
            break;
        };

        let visible = &text[..idx];
        transcript.push_str(visible);
        turns.push(ChatTurn { role: ChatRole::Assistant, content: visible.trim_end().to_string() });
        calls_used += 1;

        let result = if calls_used > MAX_TOOL_CALLS {
            "Tool budget exhausted. Answer now from what you already have; do not emit more marrow-tool blocks."
                .to_string()
        } else {
            match parse_tool_call(&extract_tool_json(visible)) {
                Err(e) => format!(
                    "Invalid marrow-tool block ({e}). Use exactly one of the documented tool JSON shapes, or answer without tools."
                ),
                Ok(call) => {
                    on(StreamUpdate::Status(Some(call.status_label())));
                    execute_tool(github, target, &call).await
                }
            }
        };

        let remaining = MAX_TOOL_CALLS.saturating_sub(calls_used);
        turns.push(ChatTurn {
            role: ChatRole::User,
            content: format!(
                "[marrow-tool result]\n{result}\n\n({remaining} tool calls remaining for this question.) Continue your answer — do not repeat the result verbatim and do not re-run the same call."
            ),
        });

        // Separator so the next segment doesn't run into the fence.
        on(StreamUpdate::Delta("\n\n".to_string()));
        transcript.push_str("\n\n");
    }

    let transcript = transcript.trim_end().to_string();
    if transcript.trim().is_empty() {
        Err("AI returned an empty response".to_string())
    } else {
        Ok(transcript)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fence(body: &str) -> String {
        format!("```marrow-tool\n{body}\n```\n")
    }

    // ── find_tool_fence_end / find_tool_fence_end_final ────────────────────

    #[test]
    fn no_fence_in_plain_text() {
        assert_eq!(find_tool_fence_end("just some prose\nmore prose\n"), None);
        assert_eq!(find_tool_fence_end_final("just some prose\nmore prose\n"), None);
    }

    #[test]
    fn unclosed_opener_is_none() {
        let s = "```marrow-tool\n{\"tool\":\"list_dir\",\"path\":\"\"}\n";
        assert_eq!(find_tool_fence_end(s), None);
        assert_eq!(find_tool_fence_end_final(s), None);
    }

    #[test]
    fn closed_marrow_action_fence_does_not_match() {
        let s = "```marrow-action\n{\"action\":\"open_overview\"}\n```\n";
        assert_eq!(find_tool_fence_end(s), None);
        assert_eq!(find_tool_fence_end_final(s), None);
    }

    #[test]
    fn marrow_tool_text_inside_a_regular_fence_does_not_match() {
        let s = "```text\nsee ```marrow-tool for details\n```\n";
        assert_eq!(find_tool_fence_end(s), None);
        assert_eq!(find_tool_fence_end_final(s), None);
    }

    #[test]
    fn complete_fence_cuts_just_past_closer_newline() {
        let s = fence(r#"{"tool":"list_dir","path":""}"#);
        let idx = find_tool_fence_end(&s).expect("should find complete fence");
        assert_eq!(idx, s.len());
        assert_eq!(&s[..idx], s.as_str());
    }

    #[test]
    fn closer_without_trailing_newline_needs_final_variant() {
        // No trailing "\n" after the closing ```.
        let s = "```marrow-tool\n{\"tool\":\"list_dir\",\"path\":\"\"}\n```";
        assert_eq!(find_tool_fence_end(s), None);
        let idx = find_tool_fence_end_final(s).expect("final variant should close at EOF");
        assert_eq!(idx, s.len());
    }

    #[test]
    fn trailing_prose_after_closer_excluded_from_cut() {
        let s = format!("{}and then some prose after.\n", fence(r#"{"tool":"list_dir","path":""}"#));
        let idx = find_tool_fence_end(&s).expect("should find complete fence");
        assert!(idx < s.len());
        assert!(s[idx..].starts_with("and then some prose"));
    }

    // ── parse_tool_call ──────────────────────────────────────────────────

    #[test]
    fn parses_the_three_valid_shapes() {
        assert_eq!(
            parse_tool_call(r#"{"tool":"read_file","path":"src/lib.rs"}"#),
            Ok(ToolCall::ReadFile { path: "src/lib.rs".to_string() })
        );
        assert_eq!(
            parse_tool_call(r#"{"tool":"search_code","query":"fn foo"}"#),
            Ok(ToolCall::SearchCode { query: "fn foo".to_string() })
        );
        assert_eq!(
            parse_tool_call(r#"{"tool":"list_dir","path":"src"}"#),
            Ok(ToolCall::ListDir { path: "src".to_string() })
        );
        // path defaults to "" for list_dir (repo root).
        assert_eq!(
            parse_tool_call(r#"{"tool":"list_dir"}"#),
            Ok(ToolCall::ListDir { path: String::new() })
        );
    }

    #[test]
    fn rejects_unknown_tool() {
        assert!(parse_tool_call(r#"{"tool":"delete_repo","path":"x"}"#).is_err());
    }

    #[test]
    fn rejects_missing_field() {
        assert!(parse_tool_call(r#"{"tool":"read_file"}"#).is_err());
        assert!(parse_tool_call(r#"{"tool":"search_code"}"#).is_err());
    }

    #[test]
    fn rejects_empty_path_or_query() {
        assert!(parse_tool_call(r#"{"tool":"read_file","path":""}"#).is_err());
        assert!(parse_tool_call(r#"{"tool":"read_file","path":"   "}"#).is_err());
        assert!(parse_tool_call(r#"{"tool":"search_code","query":""}"#).is_err());
    }

    #[test]
    fn rejects_extra_field() {
        assert!(parse_tool_call(r#"{"tool":"read_file","path":"x","extra":1}"#).is_err());
    }

    // ── extract_tool_json ────────────────────────────────────────────────

    #[test]
    fn extract_tool_json_round_trips() {
        let body = r#"{"tool":"read_file","path":"src/lib.rs"}"#;
        let s = fence(body);
        let idx = find_tool_fence_end(&s).unwrap();
        assert_eq!(extract_tool_json(&s[..idx]), body);
        assert_eq!(parse_tool_call(&extract_tool_json(&s[..idx])).unwrap(), ToolCall::ReadFile {
            path: "src/lib.rs".to_string(),
        });
    }

    // ── status_label ─────────────────────────────────────────────────────
    // execute_tool's formatting paths need network and can't be unit-tested
    // without a mock — status_label covers the same match arms without one.

    #[test]
    fn status_labels() {
        assert_eq!(
            ToolCall::ReadFile { path: "src/lib.rs".to_string() }.status_label(),
            "Reading src/lib.rs…"
        );
        assert_eq!(
            ToolCall::SearchCode { query: "fn foo".to_string() }.status_label(),
            "Searching code for \u{201c}fn foo\u{201d}…"
        );
        assert_eq!(ToolCall::ListDir { path: "src".to_string() }.status_label(), "Listing src…");
        assert_eq!(ToolCall::ListDir { path: String::new() }.status_label(), "Listing repo root…");
    }
}
