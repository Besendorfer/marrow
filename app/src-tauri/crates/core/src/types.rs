use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum FetchStatus {
    Running,
    Done,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FetchProgress {
    pub step: u8,
    pub total_steps: u8,
    pub label: String,
    pub status: FetchStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_done: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_total: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Highlight {
    pub start_line: u64,
    pub end_line: u64,
    pub severity: String,
    pub comment: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileDiff {
    pub path: String,
    pub classification: String,
    pub reason: String,
    pub category: String,
    #[serde(default = "default_risk_level")]
    pub risk_level: String,
    pub diff_type: String,
    pub base_content: String,
    pub head_content: String,
    pub unified_diff: String,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    #[serde(default)]
    pub highlights: Vec<Highlight>,
    #[serde(default)]
    pub hunk_scores: Vec<String>,
    #[serde(default)]
    pub diff_hash: String,
}

fn default_risk_level() -> String {
    "medium".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChangeGroup {
    pub label: String,
    pub description: String,
    pub file_paths: Vec<String>,
}

/// One of the 2-3 highest-risk changes, surfaced in the top-of-review triage card.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TopRisk {
    /// Short headline (e.g. "Admin-only check removed on /users").
    pub title: String,
    /// One sentence on why it carries risk.
    pub detail: String,
    /// File the reviewer should jump to.
    pub path: String,
    /// Line (in the head version) to scroll to, when known.
    #[serde(default)]
    pub start_line: Option<u64>,
}

/// One file in the contract-first "fastest path" ordering, with a one-line
/// rationale ("defines the shape the rest consumes").
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReviewOrderItem {
    pub path: String,
    #[serde(default)]
    pub rationale: String,
}

/// Triage guidance for large PRs: what to review first and in what order.
/// Absent for small PRs (see the gate in `fetch`).
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct TriageReport {
    #[serde(default)]
    pub top_risks: Vec<TopRisk>,
    #[serde(default)]
    pub review_order: Vec<ReviewOrderItem>,
}

/// One commit in a PR's commit list (the "commits" tab).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PrCommit {
    pub sha: String,
    pub message_headline: String,
    #[serde(default)]
    pub author_login: Option<String>,
    #[serde(default)]
    pub author_avatar: Option<String>,
    pub committed_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReviewManifest {
    pub pr_title: String,
    pub pr_url: String,
    pub pr_number: u64,
    pub base_ref: String,
    pub head_ref: String,
    pub base_sha: String,
    pub head_sha: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub change_groups: Vec<ChangeGroup>,
    /// Triage-first guidance (top risks + contract-first order). `None` for small
    /// PRs or when the triage AI pass fails without a usable fallback.
    #[serde(default)]
    pub triage: Option<TriageReport>,
    /// The PR description, truncated char-safely to bound cache size. Empty
    /// when the PR has no body or on manifests fetched before this field existed.
    #[serde(default)]
    pub body: String,
    /// This PR's commits, oldest first. Empty on manifests fetched before this
    /// field existed, or when the commits fetch failed (best-effort).
    #[serde(default)]
    pub commits: Vec<PrCommit>,
    pub files: Vec<FileDiff>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Settings {
    pub model: String,
    #[serde(default)]
    pub github_token: String,
    #[serde(default)]
    pub aws_profile: String,
    /// Anthropic API key for the direct-API backend (used when `model` is a
    /// model name and this/`ANTHROPIC_API_KEY` is set). No AWS/CLI needed.
    #[serde(default)]
    pub anthropic_api_key: String,
    /// Explicit provider override (anthropic / openai / gemini / bedrock /
    /// claude-cli / openai-compatible). Empty = auto-detect from the model name.
    #[serde(default)]
    pub provider: String,
    /// OpenAI (and OpenAI-compatible: OpenRouter, local, …) API key.
    #[serde(default)]
    pub openai_api_key: String,
    /// Google Gemini API key (used via Gemini's OpenAI-compatible endpoint).
    #[serde(default)]
    pub gemini_api_key: String,
    /// Base URL for the OpenAI-compatible backend (e.g. OpenRouter or a local
    /// server). Empty = OpenAI's default. Setting it implies an OpenAI-compatible
    /// provider unless overridden.
    #[serde(default)]
    pub openai_base_url: String,
    #[serde(default = "default_true")]
    pub filter_older: bool,
    #[serde(default = "default_true")]
    pub filter_team: bool,
    #[serde(default = "default_split")]
    pub view_mode: String,
    #[serde(default = "default_true")]
    pub show_hunk_significance: bool,
    #[serde(default = "default_true")]
    pub show_ai_notes: bool,
    #[serde(default = "default_all")]
    pub hunk_filter: String,
    /// Max PRs the activity mini-player surfaces per watch; the rest are
    /// reported as "+N more". Useful to raise for org-wide watches.
    #[serde(default = "default_per_watch_cap")]
    pub activity_per_watch_cap: u64,
    /// Whether the floating mini-player auto-shows when the main window loses
    /// focus. The widget's ✕ turns this off; the dock's ⧉ toggle turns it on.
    #[serde(default = "default_true")]
    pub activity_mini_player: bool,
    /// Keep PRs in the activity feed after you've approved them. Off by default:
    /// once you approve a PR, it drops out of the feed.
    #[serde(default)]
    pub show_approved_prs: bool,
    /// Whether the review queue shows draft PRs. On by default (current
    /// behavior); the frontend filters draft rows out when this is off.
    #[serde(default = "default_true")]
    pub show_draft_prs: bool,
    /// True once the first-run welcome has been completed or skipped, so it
    /// never auto-shows again (config via env vars alone also suppresses it).
    #[serde(default)]
    pub setup_done: bool,
    /// When true, files open with every hunk expanded instead of auto-collapsing
    /// low-significance hunks (issue #55). Off by default to keep the
    /// collapsed-by-default behavior.
    #[serde(default)]
    pub expand_all_hunks: bool,
}

fn default_true() -> bool {
    true
}

fn default_per_watch_cap() -> u64 {
    50
}

fn default_split() -> String {
    "split".to_string()
}

fn default_all() -> String {
    "all".to_string()
}

#[derive(Debug, Deserialize)]
pub struct FileClassification {
    pub path: String,
    pub classification: String,
    #[serde(default)]
    pub category: String,
    #[serde(default = "default_risk_level")]
    pub risk_level: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Deserialize)]
pub struct HighlightResult {
    pub path: String,
    pub start_line: u64,
    pub end_line: u64,
    #[serde(default = "default_info")]
    pub severity: String,
    #[serde(default)]
    pub comment: String,
}

fn default_info() -> String {
    "info".to_string()
}

#[derive(Debug, Deserialize)]
pub struct PrMetadata {
    pub title: String,
    pub html_url: String,
    pub number: u64,
    pub base: PrRef,
    pub head: PrRef,
    #[serde(default)]
    pub user: Option<PrUser>,
    #[serde(default)]
    pub draft: Option<bool>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PrUser {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct PrRef {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub sha: String,
}

#[derive(Debug, Deserialize)]
pub struct PrFile {
    pub filename: String,
    pub status: String,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
}

/// One file changed by a single commit (the "commit diff" side panel).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommitDiffFile {
    pub path: String,
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    /// Absent for large or binary files GitHub doesn't return a patch for.
    #[serde(default)]
    pub patch: Option<String>,
    /// The prior path, when this file was renamed.
    #[serde(default)]
    pub previous_path: Option<String>,
}

/// The diff for a single commit, fetched on demand when a commit is opened.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommitDiff {
    pub sha: String,
    pub message_headline: String,
    pub files: Vec<CommitDiffFile>,
    /// True when GitHub's 300-file cap on this endpoint truncated the list.
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommentAuthor {
    pub login: String,
    pub avatar_url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReactionGroup {
    pub content: String,
    pub total_count: u32,
    pub viewer_has_reacted: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReviewComment {
    pub id: String,
    pub body: String,
    pub author: CommentAuthor,
    pub created_at: String,
    pub updated_at: String,
    pub url: String,
    #[serde(default)]
    pub reactions: Vec<ReactionGroup>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReviewThread {
    pub id: String,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub path: String,
    pub line: Option<u64>,
    pub original_line: Option<u64>,
    pub diff_hunk: String,
    pub comments: Vec<ReviewComment>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PrUpdateStatus {
    pub has_changes: bool,
    pub head_sha_changed: bool,
    pub comment_count_changed: bool,
    pub new_head_sha: Option<String>,
    pub new_comment_count: Option<u32>,
    /// Whether the PR is now merged. Independent of `has_changes` (a merge moves
    /// neither head SHA nor comment count), so the GUI checks it separately.
    pub merged: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MyReviewState {
    pub status: String,
    pub is_re_requested: bool,
    pub is_merged: bool,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub approved_by: Vec<String>,
    /// Lowercased GitHub `mergeable` enum: "mergeable" | "conflicting" |
    /// "unknown". Empty when GitHub hasn't computed it or the field is absent.
    #[serde(default)]
    pub mergeable: String,
    #[serde(default)]
    pub labels: Vec<PrLabel>,
    /// The SHA the viewer's most recent review was submitted against, and when.
    /// `None` if the viewer hasn't reviewed this PR.
    #[serde(default)]
    pub last_reviewed_sha: Option<String>,
    #[serde(default)]
    pub last_reviewed_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PrLabel {
    pub name: String,
    /// Hex color without the leading '#', as GitHub returns it.
    #[serde(default)]
    pub color: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CheckRunInfo {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub details_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PrChecksStatus {
    pub overall_state: String,
    pub check_runs: Vec<CheckRunInfo>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CheckAnnotation {
    pub path: String,
    pub start_line: u64,
    pub end_line: u64,
    pub annotation_level: String,
    pub message: String,
    #[serde(default)]
    pub title: Option<String>,
    pub check_name: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CheckFailures {
    pub head_sha: String,
    pub annotations: Vec<CheckAnnotation>,
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReviewRequestItem {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub author: String,
    pub created_at: String,
    pub updated_at: String,
    pub draft: bool,
    pub direct_request: bool,
    pub my_review_status: String,
    pub unresolved_thread_count: u32,
    #[serde(default)]
    pub approval_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `ReviewManifest` JSON blob missing `commits` (a cached manifest saved
    /// before the field existed) must still deserialize, with `commits`
    /// defaulting to empty — mirrors `triage`'s cache-compat guarantee.
    #[test]
    fn review_manifest_defaults_commits_when_absent() {
        let json = serde_json::json!({
            "pr_title": "t",
            "pr_url": "https://github.com/o/r/pull/1",
            "pr_number": 1,
            "base_ref": "main",
            "head_ref": "feature",
            "base_sha": "aaa",
            "head_sha": "bbb",
            "files": [],
        });

        let manifest: ReviewManifest = serde_json::from_value(json).unwrap();
        assert!(manifest.commits.is_empty());
    }
}
