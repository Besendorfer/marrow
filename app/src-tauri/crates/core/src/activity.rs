//! PR activity model for the mini-player widget.
//!
//! Two responsibilities:
//!  1. Persist a per-PR *seen-state* (`activity.json`) — a snapshot of what the
//!     observable fields looked like the last time the user acknowledged a PR.
//!  2. Diff freshly observed PRs against that seen-state to produce
//!     [`PrActivityItem`]s carrying `deltas` (what changed) and an `unread` flag.
//!
//! This module is pure/testable: the network fetches live in `github.rs` and the
//! Tauri event emission lives in the app layer. [`compute_activity`] takes
//! already-fetched observations plus the loaded seen-state and returns the
//! payload to emit.

use crate::config::app_config_dir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// The observable fields of a PR we compare across polls to detect activity.
/// All optional beyond `updated_at` because different sources surface different
/// amounts of detail (a watch search gives `updated_at`; an enriched review
/// request adds review state and threads; a focused diff adds sha/comments/CI).
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct Observed {
    pub updated_at: String,
    #[serde(default)]
    pub review_state: Option<String>,
    #[serde(default)]
    pub unresolved_threads: Option<u32>,
    #[serde(default)]
    pub head_sha: Option<String>,
    #[serde(default)]
    pub comment_count: Option<u32>,
    #[serde(default)]
    pub ci_state: Option<String>,
}

/// One entry in `activity.json`: what we last *acknowledged* for a PR, plus when
/// the user marked it seen.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SeenState {
    pub last_seen_at: String,
    pub observed: Observed,
}

/// `activity.json` on disk: `pr_url -> SeenState`.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ActivityStore {
    #[serde(default)]
    pub prs: HashMap<String, SeenState>,
}

/// A PR observed this poll, with the source-derived metadata the UI renders and
/// the raw observable state used for diffing. Built by the caller from GitHub
/// results; `deltas`/`unread` are filled in by [`compute_activity`].
#[derive(Debug, Clone)]
pub struct ObservedPr {
    pub pr_url: String,
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub author: String,
    pub avatar_url: String,
    pub draft: bool,
    /// Why this PR is in the feed, e.g. "review-requested", "watching:acme-web".
    pub reasons: Vec<String>,
    pub observed: Observed,
    /// Login of the latest comment's author, filled in by enrichment. Transient
    /// (never persisted): used to suppress an "updated" delta when the viewer's
    /// own comment is what bumped `updated_at`.
    pub last_actor: Option<String>,
    /// Whether a re-review is currently requested from the viewer, filled in by
    /// enrichment. Transient: lets the approved-filter keep an approved-but-
    /// re-requested PR visible (it wants a fresh look) instead of hiding it.
    pub is_re_requested: bool,
}

/// A feed row emitted to the UI. Field names are camelCase on the wire to match
/// the existing `ReviewRequestItem` shape the frontend already renders.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PrActivityItem {
    pub pr_url: String,
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub author: String,
    pub avatar_url: String,
    pub updated_at: String,
    pub draft: bool,
    pub reasons: Vec<String>,
    /// What changed since the user last acknowledged this PR.
    pub deltas: Vec<String>,
    pub review_state: Option<String>,
    pub unresolved_threads: Option<u32>,
    pub ci_state: Option<String>,
    pub unread: bool,
}

/// The `pr-activity` event payload broadcast to all windows.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PrActivityPayload {
    pub items: Vec<PrActivityItem>,
    /// Per-watch counts of results dropped by the cap, for honest "+N more" UI.
    pub truncated: HashMap<String, u32>,
    pub fetched_at: String,
}

fn activity_path() -> PathBuf {
    app_config_dir().join("activity.json")
}

/// Current time as an RFC3339 string — the timestamp format used throughout
/// (so the app layer doesn't need its own `chrono` dependency).
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn load_activity_store() -> ActivityStore {
    let path = activity_path();
    let Ok(content) = fs::read_to_string(&path) else {
        return ActivityStore::default();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

pub fn save_activity_store(store: &ActivityStore) -> Result<(), String> {
    let path = activity_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create config dir: {}", e))?;
    }
    let json = serde_json::to_string_pretty(store)
        .map_err(|e| format!("Failed to serialize activity store: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("Failed to write activity store: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(&path, perms);
    }

    Ok(())
}

/// Compute the deltas between what we last acknowledged (`seen`) and what we
/// just observed (`now`). Empty when nothing meaningful changed.
fn diff(seen: &Observed, now: &Observed) -> Vec<String> {
    let mut deltas = Vec::new();
    if now.updated_at != seen.updated_at {
        deltas.push("updated".to_string());
    }
    if now.head_sha.is_some() && now.head_sha != seen.head_sha {
        deltas.push("new-commits".to_string());
    }
    if let (Some(now_c), Some(seen_c)) = (now.comment_count, seen.comment_count) {
        if now_c > seen_c {
            deltas.push("new-comments".to_string());
        }
    }
    // Only when we previously knew a state to compare against. A None→known
    // transition is "first time we learned it" (e.g. the first poll after
    // enrichment starts populating review_state for watch PRs), not a change —
    // flagging it would mark the whole backlog "review changed" at once.
    if now.review_state.is_some()
        && seen.review_state.is_some()
        && now.review_state != seen.review_state
    {
        deltas.push("review-state-changed".to_string());
    }
    if let (Some(now_t), Some(seen_t)) = (now.unresolved_threads, seen.unresolved_threads) {
        if now_t > seen_t {
            deltas.push("new-threads".to_string());
        }
    }
    if now.ci_state.is_some() && now.ci_state != seen.ci_state {
        deltas.push("ci-changed".to_string());
    }
    deltas
}

/// Whether the PR's latest comment was authored by the viewer (case-insensitive).
/// `viewer` empty (e.g. no token) ⇒ never, so behavior is unchanged.
fn is_self_authored(pr: &ObservedPr, viewer: &str) -> bool {
    !viewer.is_empty()
        && pr
            .last_actor
            .as_deref()
            .map(|a| a.eq_ignore_ascii_case(viewer))
            .unwrap_or(false)
}

/// Turn this poll's observations into a feed payload by diffing against the
/// persisted seen-state. A PR never seen before is `unread` with a `new` delta;
/// a PR whose observable fields all match its seen-state is read with no deltas.
pub fn compute_activity(
    observations: Vec<ObservedPr>,
    store: &ActivityStore,
    truncated: HashMap<String, u32>,
    fetched_at: String,
    viewer: &str,
    show_approved: bool,
) -> PrActivityPayload {
    let mut items: Vec<PrActivityItem> = observations
        .into_iter()
        // Once you've approved a PR, drop it from the feed unless you opt to keep
        // approved PRs — but always keep one whose review is freshly re-requested.
        .filter(|pr| {
            show_approved
                || pr.is_re_requested
                || pr.observed.review_state.as_deref() != Some("approved")
        })
        .map(|pr| {
            let (deltas, unread) = match store.prs.get(&pr.pr_url) {
                Some(seen) => {
                    let mut d = diff(&seen.observed, &pr.observed);
                    // Your own comment bumps `updated_at` but isn't news: drop the
                    // generic "updated" delta when the latest comment was yours.
                    // Concrete deltas (new commits, review-state, CI, threads)
                    // still stand on their own.
                    if is_self_authored(&pr, viewer) {
                        d.retain(|x| x != "updated");
                    }
                    let unread = !d.is_empty();
                    (d, unread)
                }
                None => (vec!["new".to_string()], true),
            };
            PrActivityItem {
                pr_url: pr.pr_url,
                owner: pr.owner,
                repo: pr.repo,
                number: pr.number,
                title: pr.title,
                author: pr.author,
                avatar_url: pr.avatar_url,
                updated_at: pr.observed.updated_at.clone(),
                draft: pr.draft,
                reasons: pr.reasons,
                deltas,
                review_state: pr.observed.review_state.clone(),
                unresolved_threads: pr.observed.unresolved_threads,
                ci_state: pr.observed.ci_state.clone(),
                unread,
            }
        })
        .collect();

    // Unread first, then most-recently-updated.
    items.sort_by(|a, b| {
        b.unread
            .cmp(&a.unread)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });

    PrActivityPayload {
        items,
        truncated,
        fetched_at,
    }
}

/// Record that the user has acknowledged a PR: store its current observable
/// state so future polls diff against it (clearing the unread state).
pub fn mark_seen(store: &mut ActivityStore, pr_url: &str, observed: Observed, now: String) {
    store.prs.insert(
        pr_url.to_string(),
        SeenState {
            last_seen_at: now,
            observed,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(updated: &str) -> Observed {
        Observed {
            updated_at: updated.to_string(),
            ..Default::default()
        }
    }

    fn pr(url: &str, observed: Observed) -> ObservedPr {
        ObservedPr {
            pr_url: url.to_string(),
            owner: "o".into(),
            repo: "r".into(),
            number: 1,
            title: "t".into(),
            author: "a".into(),
            avatar_url: "".into(),
            draft: false,
            reasons: vec!["watching:x".into()],
            observed,
            last_actor: None,
            is_re_requested: false,
        }
    }

    #[test]
    fn never_seen_pr_is_unread_with_new_delta() {
        let store = ActivityStore::default();
        let payload = compute_activity(
            vec![pr("u1", obs("2026-01-01T00:00:00Z"))],
            &store,
            HashMap::new(),
            "now".into(),
            "",
            true,
        );
        assert_eq!(payload.items.len(), 1);
        assert!(payload.items[0].unread);
        assert_eq!(payload.items[0].deltas, vec!["new".to_string()]);
    }

    #[test]
    fn unchanged_pr_is_read_with_no_deltas() {
        let mut store = ActivityStore::default();
        mark_seen(&mut store, "u1", obs("2026-01-01T00:00:00Z"), "seen".into());
        let payload = compute_activity(
            vec![pr("u1", obs("2026-01-01T00:00:00Z"))],
            &store,
            HashMap::new(),
            "now".into(),
            "",
            true,
        );
        assert!(!payload.items[0].unread);
        assert!(payload.items[0].deltas.is_empty());
    }

    #[test]
    fn newer_update_marks_unread() {
        let mut store = ActivityStore::default();
        mark_seen(&mut store, "u1", obs("2026-01-01T00:00:00Z"), "seen".into());
        let payload = compute_activity(
            vec![pr("u1", obs("2026-02-01T00:00:00Z"))],
            &store,
            HashMap::new(),
            "now".into(),
            "",
            true,
        );
        assert!(payload.items[0].unread);
        assert!(payload.items[0].deltas.contains(&"updated".to_string()));
    }

    #[test]
    fn detects_richer_field_changes() {
        let seen = Observed {
            updated_at: "t".into(),
            review_state: Some("pending".into()),
            unresolved_threads: Some(1),
            head_sha: Some("aaa".into()),
            comment_count: Some(3),
            ci_state: Some("success".into()),
        };
        let now = Observed {
            updated_at: "t".into(), // same timestamp; only richer fields move
            review_state: Some("changes_requested".into()),
            unresolved_threads: Some(3),
            head_sha: Some("bbb".into()),
            comment_count: Some(5),
            ci_state: Some("failure".into()),
        };
        let d = diff(&seen, &now);
        assert!(d.contains(&"review-state-changed".to_string()));
        assert!(d.contains(&"new-threads".to_string()));
        assert!(d.contains(&"new-commits".to_string()));
        assert!(d.contains(&"new-comments".to_string()));
        assert!(d.contains(&"ci-changed".to_string()));
        assert!(!d.contains(&"updated".to_string()));
    }

    #[test]
    fn own_comment_does_not_mark_unread() {
        let mut store = ActivityStore::default();
        mark_seen(&mut store, "u1", obs("2026-01-01T00:00:00Z"), "seen".into());
        // updated_at moved, but the latest comment is the viewer's own.
        let mut p = pr("u1", obs("2026-02-01T00:00:00Z"));
        p.last_actor = Some("Me".into());
        let payload = compute_activity(vec![p], &store, HashMap::new(), "now".into(), "me", true);
        assert!(!payload.items[0].unread, "own comment should not be unread");
        assert!(!payload.items[0].deltas.contains(&"updated".to_string()));
    }

    #[test]
    fn others_comment_still_marks_unread() {
        let mut store = ActivityStore::default();
        mark_seen(&mut store, "u1", obs("2026-01-01T00:00:00Z"), "seen".into());
        let mut p = pr("u1", obs("2026-02-01T00:00:00Z"));
        p.last_actor = Some("someone-else".into());
        let payload = compute_activity(vec![p], &store, HashMap::new(), "now".into(), "me", true);
        assert!(payload.items[0].unread);
        assert!(payload.items[0].deltas.contains(&"updated".to_string()));
    }

    #[test]
    fn own_comment_still_unread_when_a_concrete_delta_moved() {
        let mut store = ActivityStore::default();
        let seen = Observed {
            updated_at: "2026-01-01T00:00:00Z".into(),
            head_sha: Some("aaa".into()),
            ..Default::default()
        };
        mark_seen(&mut store, "u1", seen, "seen".into());
        // My own comment bumped updated_at, but a new commit also landed.
        let now = Observed {
            updated_at: "2026-02-01T00:00:00Z".into(),
            head_sha: Some("bbb".into()),
            ..Default::default()
        };
        let mut p = pr("u1", now);
        p.last_actor = Some("me".into());
        let payload = compute_activity(vec![p], &store, HashMap::new(), "now".into(), "me", true);
        assert!(payload.items[0].unread, "new commits keep it unread");
        assert!(payload.items[0].deltas.contains(&"new-commits".to_string()));
        assert!(!payload.items[0].deltas.contains(&"updated".to_string()));
    }

    #[test]
    fn first_known_review_state_is_not_a_change() {
        // seen had no review_state (pre-enrichment); now enrichment supplies one.
        let seen = Observed {
            updated_at: "t".into(),
            review_state: None,
            ..Default::default()
        };
        let now = Observed {
            updated_at: "t".into(),
            review_state: Some("pending".into()),
            ..Default::default()
        };
        let d = diff(&seen, &now);
        assert!(
            !d.contains(&"review-state-changed".to_string()),
            "None→known is not a change"
        );
    }

    fn approved(url: &str, re_requested: bool) -> ObservedPr {
        let mut p = pr(url, obs("2026-01-01T00:00:00Z"));
        p.observed.review_state = Some("approved".into());
        p.is_re_requested = re_requested;
        p
    }

    #[test]
    fn approved_prs_are_filtered_unless_opted_in_or_re_requested() {
        let store = ActivityStore::default();
        // Default (show_approved = false): a plain approved PR is dropped, but an
        // approved PR with a re-review requested stays.
        let payload = compute_activity(
            vec![approved("approved-url", false), approved("re-req-url", true)],
            &store,
            HashMap::new(),
            "now".into(),
            "",
            false,
        );
        let urls: Vec<&str> = payload.items.iter().map(|i| i.pr_url.as_str()).collect();
        assert_eq!(urls, vec!["re-req-url"], "approved dropped, re-requested kept");

        // show_approved = true keeps both.
        let payload = compute_activity(
            vec![approved("approved-url", false)],
            &store,
            HashMap::new(),
            "now".into(),
            "",
            true,
        );
        assert_eq!(payload.items.len(), 1, "show_approved keeps approved PRs");
    }

    #[test]
    fn unread_items_sort_first() {
        let mut store = ActivityStore::default();
        mark_seen(&mut store, "seen-url", obs("2026-05-01T00:00:00Z"), "s".into());
        let payload = compute_activity(
            vec![
                pr("seen-url", obs("2026-05-01T00:00:00Z")), // read
                pr("new-url", obs("2026-01-01T00:00:00Z")),  // unread, older
            ],
            &store,
            HashMap::new(),
            "now".into(),
            "",
            true,
        );
        assert_eq!(payload.items[0].pr_url, "new-url", "unread sorts before read");
    }
}
