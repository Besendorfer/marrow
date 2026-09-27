//! The chat's agentic tool-use loop (issue #150): lets the model call
//! read-only repo tools mid-answer via a ```marrow-tool fenced block (see
//! `CHAT_REPO_TOOLS` in chat.rs, the single source of truth for the
//! protocol). Provider-agnostic — it works the same for every `AiBackend`
//! variant because it only relies on `invoke_chat_stream`'s streaming
//! contract (Delta/Status updates, cancel-on-drop), never a provider-specific
//! tool-calling API. Execution is backend-only: the frontend (RichText.tsx)
//! only renders the fence as a chip, unlike `marrow-action`.

use crate::ai::{AiBackend, ChatRole, ChatTurn, StreamUpdate};
use crate::github::GithubClient;
pub use crate::repo_tools::RepoToolTarget;
#[cfg(test)]
use crate::repo_tools::{sanitize_search_query, validate_repo_path, ToolCall};
use crate::repo_tools::{parse_tool_call, ToolBackend, ToolExecutor, ToolScope};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// Chat's per-question tool budget.
const MAX_TOOL_CALLS: usize = 5;

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

/// Per-segment streaming state shared between the stream callback and the
/// fence-completion race in [`run_chat_agent`].
struct Seg {
    text: String,
    forwarded: usize,
    cut: Option<usize>,
}

/// Drive one chat answer through the tool-use loop against the PR repo at
/// its head (chat scope, `MAX_TOOL_CALLS` calls). The returned transcript
/// (which includes the marrow-tool fences themselves) is what the caller
/// sends as `Done { content }` and persists to history — fences render as
/// chips in the saved transcript, exactly like `marrow-action`/`marrow-card`.
pub async fn run_chat_agent(
    backend: &AiBackend,
    github: &GithubClient,
    target: &RepoToolTarget,
    system: &str,
    turns: Vec<ChatTurn>,
    on: &mut (dyn FnMut(StreamUpdate) + Send),
) -> Result<String, String> {
    let executor = ToolExecutor::new(
        ToolBackend::Github(github),
        RepoToolTarget {
            owner: target.owner.clone(),
            repo: target.repo.clone(),
            head_sha: target.head_sha.clone(),
            base_sha: String::new(),
        },
        ToolScope::CHAT,
    );
    run_agent(backend, &executor, system, turns, MAX_TOOL_CALLS, on).await.map(|r| r.transcript)
}

/// Outcome of one [`run_agent`] loop.
pub struct AgentRun {
    /// Every segment, fences included (chat persists this).
    pub transcript: String,
    /// The model's last segment — the text after its final tool call (the
    /// review parses its JSON answer from here, never from a fence).
    pub final_segment: String,
    /// Tool calls the model attempted (valid or not).
    pub tool_calls: usize,
}

/// The tool-use loop shared by chat and the review pass (issue #232):
/// stream text, abort the underlying call the instant a ```marrow-tool
/// fence completes, execute the tool via `executor`, feed the result back as
/// a turn, and re-invoke — up to `max_calls` executed calls. Provider-
/// agnostic: it relies only on `invoke_chat_stream`'s streaming contract.
pub async fn run_agent(
    backend: &AiBackend,
    executor: &ToolExecutor<'_>,
    system: &str,
    mut turns: Vec<ChatTurn>,
    max_calls: usize,
    on: &mut (dyn FnMut(StreamUpdate) + Send),
) -> Result<AgentRun, String> {
    let mut transcript = String::new();
    let mut final_segment = String::new();
    let mut calls_used: usize = 0;

    // max_calls executed calls + 1 budget-exhausted notice + the forced final segment.
    for _ in 0..(max_calls + 2) {
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
            final_segment = text;
            break;
        };

        let visible = &text[..idx];
        transcript.push_str(visible);
        turns.push(ChatTurn { role: ChatRole::Assistant, content: visible.trim_end().to_string() });
        calls_used += 1;

        let result = if calls_used > max_calls {
            "Tool budget exhausted. Answer now from what you already have; do not emit more marrow-tool blocks."
                .to_string()
        } else {
            match parse_tool_call(&extract_tool_json(visible)) {
                Err(e) => format!(
                    "Invalid marrow-tool block ({e}). Use exactly one of the documented tool JSON shapes, or answer without tools."
                ),
                Ok(call) => {
                    on(StreamUpdate::Status(Some(call.status_label())));
                    executor.execute(&call).await
                }
            }
        };

        let remaining = max_calls.saturating_sub(calls_used);
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
        Ok(AgentRun { transcript, final_segment, tool_calls: calls_used })
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
            Ok(ToolCall::ReadFile { path: "src/lib.rs".to_string(), repo: None, rev: None })
        );
        assert_eq!(
            parse_tool_call(r#"{"tool":"search_code","query":"fn foo"}"#),
            Ok(ToolCall::SearchCode { query: "fn foo".to_string(), scope: None })
        );
        assert_eq!(
            parse_tool_call(r#"{"tool":"list_dir","path":"src"}"#),
            Ok(ToolCall::ListDir { path: "src".to_string(), repo: None })
        );
        // path defaults to "" for list_dir (repo root).
        assert_eq!(
            parse_tool_call(r#"{"tool":"list_dir"}"#),
            Ok(ToolCall::ListDir { path: String::new(), repo: None })
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
            repo: None,
            rev: None,
        });
    }

    // ── status_label ─────────────────────────────────────────────────────
    #[test]
    fn sanitize_strips_scope_widening_qualifiers() {
        assert_eq!(sanitize_search_query("fn truncate repo:other/repo"), "fn truncate");
        assert_eq!(sanitize_search_query("ORG:evil USER:x needle"), "needle");
        assert_eq!(sanitize_search_query("repo:a/b"), "");
        assert_eq!(sanitize_search_query("plain query"), "plain query");
    }

    #[test]
    fn validate_repo_path_rejects_request_rewrites() {
        for bad in ["/etc", "a/../b", "..", ".", "a\\b", "src?ref=main", "src#frag", "./a"] {
            assert!(validate_repo_path(bad).is_err(), "{bad} should be rejected");
        }
        for good in ["src/lib.rs", "docs/mini-player.md", "a/b/c.txt", "with space.md"] {
            assert!(validate_repo_path(good).is_ok(), "{good} should be accepted");
        }
    }

    // execute_tool's formatting paths need network and can't be unit-tested
    // without a mock — status_label covers the same match arms without one.

    #[test]
    fn status_labels() {
        assert_eq!(
            ToolCall::ReadFile { path: "src/lib.rs".to_string(), repo: None, rev: None }.status_label(),
            "Reading src/lib.rs…"
        );
        assert_eq!(
            ToolCall::SearchCode { query: "fn foo".to_string(), scope: None }.status_label(),
            "Searching code for \u{201c}fn foo\u{201d}…"
        );
        assert_eq!(ToolCall::ListDir { path: "src".to_string(), repo: None }.status_label(), "Listing src…");
        assert_eq!(ToolCall::ListDir { path: String::new(), repo: None }.status_label(), "Listing repo root…");
    }
}
