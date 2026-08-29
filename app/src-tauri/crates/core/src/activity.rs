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
    /// Muted until the next poll's diff against `observed` finds any delta —
    /// see [`compute_activity`], which clears this the moment the PR "wakes".
    /// `#[serde(default)]` so existing `activity.json` files (written before
    /// this field existed) parse as not-snoozed rather than failing to load.
    #[serde(default)]
    pub snoozed: bool,
}

/// `activity.json` on disk: `pr_url -> SeenState`.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ActivityStore {
    #[serde(default)]
    pub prs: HashMap<String, SeenState>,
    /// Set by [`compute_activity`] when it mutated the store (a snooze woke).
    /// Lets the poll loop skip rewriting activity.json on quiet polls — the
    /// file is unlocked last-write-wins shared with mark_pr_seen/snooze_pr,
    /// so every avoided write shrinks the lost-update window.
    #[serde(skip)]
    pub dirty: bool,
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
    /// "needs_you" | "yours" | "watching" — see [`compute_tier`]. Additive:
    /// existing consumers that don't read this field are unaffected.
    #[serde(default)]
    pub tier: String,
    /// Sortable relevance score — see [`compute_urgency`]. Additive: not used
    /// by `compute_activity`'s own sort (unread desc, updated_at desc), which
    /// stays unchanged; a future queue view can sort by this instead.
    #[serde(default)]
    pub urgency: u32,
    /// Muted by the user until the next delta wakes it. Additive: current UI
    /// ignores this field, so it has no visible effect yet.
    #[serde(default)]
    pub snoozed: bool,
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
    let json = serde_json::to_string_pretty(store)
        .map_err(|e| format!("Failed to serialize activity store: {}", e))?;
    crate::state_io::write_atomic(&path, json.as_bytes())
        .map_err(|e| format!("Failed to write activity store: {}", e))?;

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
    // Same first-known-value rule as review_state: enrichment newly populating
    // ci_state (None→Some) is learning, not change — without the seen guard,
    // the first poll after CI enrichment ships would fire "ci-changed" for the
    // entire backlog at once.
    if now.ci_state.is_some() && seen.ci_state.is_some() && now.ci_state != seen.ci_state {
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

/// Whether any of `reasons` reads as "someone wants your review specifically"
/// — a direct request, or a notification whose subject is review-flavored
/// (as opposed to e.g. a generic "comment"/"subscribed"/"state_change" you're
/// only following). Used by [`compute_tier`].
fn is_review_ish_reason(reasons: &[String]) -> bool {
    reasons.iter().any(|r| {
        r == "review-requested"
            || r.strip_prefix("notification:")
                .map(|subject| matches!(subject, "review_requested" | "mention" | "team_mention"))
                .unwrap_or(false)
    })
}

/// Which of the three mini-player tiers a PR belongs to:
///  - `"yours"` — you authored it (case-insensitive login match against
///    `viewer`); takes priority even over a re-request, since it's still your
///    own PR.
///  - `"needs_you"` — a fresh re-review request, a direct `"review-requested"`
///    reason, or a review-flavored notification (`review_requested`/`mention`/
///    `team_mention`) — someone is explicitly waiting on you.
///  - `"watching"` — everything else: a saved watch, an "involved" PR you're
///    only following, or a non-review notification.
fn compute_tier(author: &str, viewer: &str, reasons: &[String], is_re_requested: bool) -> String {
    if !viewer.is_empty() && author.eq_ignore_ascii_case(viewer) {
        return "yours".to_string();
    }
    if is_re_requested || is_review_ish_reason(reasons) {
        return "needs_you".to_string();
    }
    "watching".to_string()
}

/// Sortable relevance score — highest first. This is data for the mini-player
/// widget and (later) the queue home; it does NOT change `compute_activity`'s
/// own sort (unread desc, updated_at desc), which stays exactly as before.
///
/// The ladder, highest urgency first:
///  1. A fresh re-review request (`is_re_requested`) — someone explicitly
///     asked for another look at your own past review.
///  2. Your own PR whose CI just went red — you're blocked on yourself.
///  3. An unread PR in the `needs_you` tier — someone is waiting on you and
///     you haven't acknowledged it yet.
///  4. New unresolved threads or comments since you last looked.
///  5. CI state changed (any direction, any tier not already covered above).
///  6. A plain "updated" delta — the catch-all "something moved".
///  0. Nothing changed / not urgent.
///
/// Exact weights are arbitrary; only the ordering between rungs is the
/// contract callers can rely on.
fn compute_urgency(
    tier: &str,
    unread: bool,
    is_re_requested: bool,
    ci_state: Option<&str>,
    deltas: &[String],
) -> u32 {
    if is_re_requested {
        return 100;
    }
    if tier == "yours" && matches!(ci_state, Some("failure") | Some("error")) {
        return 90;
    }
    if tier == "needs_you" && unread {
        return 80;
    }
    if deltas.iter().any(|d| d == "new-threads" || d == "new-comments") {
        return 60;
    }
    if deltas.iter().any(|d| d == "ci-changed") {
        return 40;
    }
    if deltas.iter().any(|d| d == "updated") {
        return 20;
    }
    0
}

/// Turn this poll's observations into a feed payload by diffing against the
/// persisted seen-state. A PR never seen before is `unread` with a `new` delta;
/// a PR whose observable fields all match its seen-state is read with no deltas.
///
/// `store` is mutable: a PR snoozed in a prior poll whose diff now produces
/// any delta has its persisted `snoozed` flag cleared here (it "wakes") so
/// later polls — which diff against this same seen-state — don't keep
/// reporting it as muted. Callers must persist `store` afterward (see
/// `poll_activity_once` in the desktop crate) for the wake to stick.
pub fn compute_activity(
    observations: Vec<ObservedPr>,
    store: &mut ActivityStore,
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
            let (deltas, unread, was_snoozed) = match store.prs.get(&pr.pr_url) {
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
                    (d, unread, seen.snoozed)
                }
                None => (vec!["new".to_string()], true, false),
            };
            // A delta landing while snoozed wakes the item: persist the clear
            // so future polls (diffing against this same seen-state) agree.
            let snoozed = if was_snoozed && unread {
                if let Some(seen) = store.prs.get_mut(&pr.pr_url) {
                    seen.snoozed = false;
                    store.dirty = true;
                }
                false
            } else {
                was_snoozed
            };
            let tier = compute_tier(&pr.author, viewer, &pr.reasons, pr.is_re_requested);
            let urgency = compute_urgency(
                &tier,
                unread,
                pr.is_re_requested,
                pr.observed.ci_state.as_deref(),
                &deltas,
            );
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
                tier,
                urgency,
                snoozed,
            }
        })
        .collect();

    // Unread first, then most-recently-updated. Unchanged by tier/urgency —
    // those are additive data for the widget/queue-home to sort by later.
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
/// state so future polls diff against it (clearing the unread state and any
/// snooze — an explicit acknowledgement isn't a snooze).
pub fn mark_seen(store: &mut ActivityStore, pr_url: &str, observed: Observed, now: String) {
    store.prs.insert(
        pr_url.to_string(),
        SeenState {
            last_seen_at: now,
            observed,
            snoozed: false,
        },
    );
}

/// Snooze a PR: like [`mark_seen`], but flags it `snoozed` so the UI can mute
/// it until the next poll's diff against this same seen-state finds any
/// delta, at which point `compute_activity` clears the flag (the item wakes).
pub fn snooze(store: &mut ActivityStore, pr_url: &str, observed: Observed, now: String) {
    store.prs.insert(
        pr_url.to_string(),
        SeenState {
            last_seen_at: now,
            observed,
            snoozed: true,
        },
    );
}

/// Clear a manual snooze without waiting for a delta.
pub fn unsnooze(store: &mut ActivityStore, pr_url: &str) {
    if let Some(seen) = store.prs.get_mut(pr_url) {
        seen.snoozed = false;
    }
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
        let mut store = ActivityStore::default();
        let payload = compute_activity(
            vec![pr("u1", obs("2026-01-01T00:00:00Z"))],
            &mut store,
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
            &mut store,
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
            &mut store,
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
        let payload = compute_activity(vec![p], &mut store, HashMap::new(), "now".into(), "me", true);
        assert!(!payload.items[0].unread, "own comment should not be unread");
        assert!(!payload.items[0].deltas.contains(&"updated".to_string()));
    }

    #[test]
    fn others_comment_still_marks_unread() {
        let mut store = ActivityStore::default();
        mark_seen(&mut store, "u1", obs("2026-01-01T00:00:00Z"), "seen".into());
        let mut p = pr("u1", obs("2026-02-01T00:00:00Z"));
        p.last_actor = Some("someone-else".into());
        let payload = compute_activity(vec![p], &mut store, HashMap::new(), "now".into(), "me", true);
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
        let payload = compute_activity(vec![p], &mut store, HashMap::new(), "now".into(), "me", true);
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
        let mut store = ActivityStore::default();
        // Default (show_approved = false): a plain approved PR is dropped, but an
        // approved PR with a re-review requested stays.
        let payload = compute_activity(
            vec![approved("approved-url", false), approved("re-req-url", true)],
            &mut store,
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
            &mut store,
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
            &mut store,
            HashMap::new(),
            "now".into(),
            "",
            true,
        );
        assert_eq!(payload.items[0].pr_url, "new-url", "unread sorts before read");
    }

    // ---- Tier ---------------------------------------------------------

    #[test]
    fn tier_yours_when_author_matches_viewer_case_insensitive() {
        let mut store = ActivityStore::default();
        let mut p = pr("u1", obs("t"));
        p.author = "Me".into();
        let payload = compute_activity(vec![p], &mut store, HashMap::new(), "now".into(), "me", true);
        assert_eq!(payload.items[0].tier, "yours");
    }

    #[test]
    fn tier_needs_you_on_direct_review_request() {
        let mut store = ActivityStore::default();
        let mut p = pr("u1", obs("t"));
        p.reasons = vec!["review-requested".into()];
        let payload = compute_activity(vec![p], &mut store, HashMap::new(), "now".into(), "me", true);
        assert_eq!(payload.items[0].tier, "needs_you");
    }

    #[test]
    fn tier_needs_you_on_review_ish_notification() {
        let mut store = ActivityStore::default();
        let mut p = pr("u1", obs("t"));
        p.reasons = vec!["notification:mention".into()];
        let payload = compute_activity(vec![p], &mut store, HashMap::new(), "now".into(), "me", true);
        assert_eq!(payload.items[0].tier, "needs_you");
    }

    #[test]
    fn tier_needs_you_on_re_requested() {
        let mut store = ActivityStore::default();
        let mut p = pr("u1", obs("t"));
        p.reasons = vec!["watching:x".into()];
        p.is_re_requested = true;
        let payload = compute_activity(vec![p], &mut store, HashMap::new(), "now".into(), "me", true);
        assert_eq!(payload.items[0].tier, "needs_you");
    }

    #[test]
    fn tier_watching_otherwise() {
        let mut store = ActivityStore::default();
        let mut p = pr("u1", obs("t"));
        p.reasons = vec!["notification:subscribed".into()];
        let payload = compute_activity(vec![p], &mut store, HashMap::new(), "now".into(), "me", true);
        assert_eq!(payload.items[0].tier, "watching");
    }

    // ---- Urgency -------------------------------------------------------

    #[test]
    fn urgency_ladder_ordering() {
        let mut store = ActivityStore::default();

        // Re-requested outranks everything.
        let mut re_req = pr("re-req", obs("t"));
        re_req.is_re_requested = true;

        // Own PR with red CI outranks a plain review-requested.
        let mut own_red_ci = pr("own-red-ci", obs("t"));
        own_red_ci.author = "me".into();
        own_red_ci.observed.ci_state = Some("failure".into());

        // Unread review-requested (needs_you) outranks a mere delta.
        let mut review_requested = pr("review-requested", obs("t"));
        review_requested.reasons = vec!["review-requested".into()];

        let payload = compute_activity(
            vec![re_req, own_red_ci, review_requested],
            &mut store,
            HashMap::new(),
            "now".into(),
            "me",
            true,
        );
        let urgency_of = |url: &str| {
            payload
                .items
                .iter()
                .find(|i| i.pr_url == url)
                .unwrap()
                .urgency
        };
        assert!(
            urgency_of("re-req") > urgency_of("own-red-ci"),
            "re-request outranks own-PR-red-CI"
        );
        assert!(
            urgency_of("own-red-ci") > urgency_of("review-requested"),
            "own-PR-red-CI outranks review-requested"
        );
    }

    // ---- Snooze ---------------------------------------------------------

    #[test]
    fn snoozed_pr_stays_snoozed_when_nothing_changed() {
        let mut store = ActivityStore::default();
        snooze(&mut store, "u1", obs("2026-01-01T00:00:00Z"), "s".into());
        let payload = compute_activity(
            vec![pr("u1", obs("2026-01-01T00:00:00Z"))],
            &mut store,
            HashMap::new(),
            "now".into(),
            "",
            true,
        );
        assert!(payload.items[0].snoozed, "no delta, stays snoozed");
        assert!(store.prs.get("u1").unwrap().snoozed, "persisted state unchanged");
    }

    #[test]
    fn snoozed_pr_wakes_on_any_delta() {
        let mut store = ActivityStore::default();
        snooze(&mut store, "u1", obs("2026-01-01T00:00:00Z"), "s".into());
        let payload = compute_activity(
            vec![pr("u1", obs("2026-02-01T00:00:00Z"))], // updated_at moved
            &mut store,
            HashMap::new(),
            "now".into(),
            "",
            true,
        );
        assert!(!payload.items[0].snoozed, "a delta wakes the item");
        assert!(
            !store.prs.get("u1").unwrap().snoozed,
            "the wake is persisted so future polls agree"
        );
    }

    #[test]
    fn unsnooze_clears_without_a_delta() {
        let mut store = ActivityStore::default();
        snooze(&mut store, "u1", obs("t"), "s".into());
        unsnooze(&mut store, "u1");
        assert!(!store.prs.get("u1").unwrap().snoozed);
    }

    #[test]
    fn mark_seen_clears_any_prior_snooze() {
        let mut store = ActivityStore::default();
        snooze(&mut store, "u1", obs("t"), "s".into());
        mark_seen(&mut store, "u1", obs("t2"), "s2".into());
        assert!(!store.prs.get("u1").unwrap().snoozed);
    }
}
