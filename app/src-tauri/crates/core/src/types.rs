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
    pub summary: String,
    #[serde(default)]
    pub change_groups: Vec<ChangeGroup>,
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
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MyReviewState {
    pub status: String,
    pub is_re_requested: bool,
    pub is_merged: bool,
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
}
