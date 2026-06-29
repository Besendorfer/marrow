export type RiskLevel = "critical" | "high" | "medium" | "low";

export type HighlightSeverity = "critical" | "warning" | "info";

export interface Highlight {
  start_line: number;
  end_line: number;
  severity: HighlightSeverity;
  comment: string;
}

export interface FileDiff {
  path: string;
  classification: string;
  reason: string;
  category: string;
  risk_level: RiskLevel;
  diff_type: "modified" | "added" | "removed";
  base_content: string;
  head_content: string;
  unified_diff: string;
  additions: number;
  deletions: number;
  highlights: Highlight[];
  hunk_scores: string[];
  diff_hash: string;
}

export interface ChangeGroup {
  label: string;
  description: string;
  file_paths: string[];
}

export interface ReviewManifest {
  pr_title: string;
  pr_url: string;
  pr_number: number;
  base_ref: string;
  head_ref: string;
  base_sha: string;
  head_sha: string;
  summary: string;
  change_groups: ChangeGroup[];
  files: FileDiff[];
}

export interface FetchProgress {
  step: number;
  total_steps: number;
  label: string;
  status: "running" | "done";
  pr_title?: string;
  files_done?: number;
  files_total?: number;
}

export interface CommentAuthor {
  login: string;
  avatar_url: string;
}

export interface ReactionGroup {
  content: string;
  total_count: number;
  viewer_has_reacted: boolean;
}

export interface ReviewComment {
  id: string;
  body: string;
  author: CommentAuthor;
  created_at: string;
  updated_at: string;
  url: string;
  reactions: ReactionGroup[];
}

export interface ReviewThread {
  id: string;
  is_resolved: boolean;
  is_outdated: boolean;
  path: string;
  line: number | null;
  original_line: number | null;
  diff_hunk: string;
  comments: ReviewComment[];
}

export type CommentThreadsState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "loaded"; threads: ReviewThread[] }
  | { status: "error"; message: string };

export type SidebarView = "groups" | "comments" | "category" | "tree";

export type DiffViewMode = "split" | "unified";

export type HunkSignificanceFilter = "all" | "high" | "medium" | "low";

export interface TabLoadingState {
  prRef: string;
  prTitle: string | null;
  progress: FetchProgress | null;
  fileCounts: Record<number, { done: number; total: number }>;
}

export interface Tab {
  id: string;
  /** null while the tab is an opener/loading tab that hasn't loaded a PR yet */
  manifest: ReviewManifest | null;
  /** present while this tab is actively fetching a PR; null/absent otherwise */
  loading?: TabLoadingState | null;
  /** true when the tab finished (loaded or errored) while inactive; cleared when viewed */
  unread?: boolean;
  /** error message from a failed fetch in this (still pending) tab; null otherwise */
  error?: string | null;
  selectedFile: FileDiff | null;
  viewedFiles: Set<string>;
  staleViewedFiles: Set<string>;
  /** Keys (see highlightKey) of AI highlights the user has dismissed for this PR. */
  dismissedHighlights: Set<string>;
  commentThreads: CommentThreadsState;
  selectedCommentFile: string | null;
  sidebarView: SidebarView;
  isRefreshing?: boolean;
  lastCommentCount?: number;
  myReviewState?: MyReviewState;
}

export interface PrUpdateStatus {
  has_changes: boolean;
  head_sha_changed: boolean;
  comment_count_changed: boolean;
  new_head_sha: string | null;
  new_comment_count: number | null;
}

export interface SearchMatch {
  filePath: string;
  lineNumber: number;
  lineContent: string;
  matchStart: number;
  matchLength: number;
}

export interface Settings {
  model: string;
  github_token: string;
  aws_profile: string;
  anthropic_api_key: string;
  provider: string;
  openai_api_key: string;
  gemini_api_key: string;
  openai_base_url: string;
  filter_older: boolean;
  filter_team: boolean;
  view_mode: DiffViewMode;
  show_hunk_significance: boolean;
  show_ai_notes: boolean;
  hunk_filter: HunkSignificanceFilter;
  activity_per_watch_cap: number;
  activity_mini_player: boolean;
  show_approved_prs: boolean;
}

export type ReviewStatus = "approved" | "changes_requested" | "commented" | "dismissed" | "pending";

export interface MyReviewState {
  status: ReviewStatus;
  is_re_requested: boolean;
  is_merged: boolean;
}

export interface CheckRunInfo {
  name: string;
  status: string;
  conclusion: string | null;
  details_url: string | null;
}

export interface PrChecksStatus {
  overall_state: string;
  check_runs: CheckRunInfo[];
}

export interface ViewedFileState {
  files: Record<string, string>; // path -> diff_hash
}

export interface CachedPrInfo {
  owner: string;
  repo: string;
  pr_number: number;
  pr_title: string;
  pr_url: string;
  head_sha: string;
  file_count: number;
  cached_at: string;
}

export interface ReviewRequestItem {
  owner: string;
  repo: string;
  number: number;
  title: string;
  html_url: string;
  author: string;
  created_at: string;
  updated_at: string;
  draft: boolean;
  direct_request: boolean;
  my_review_status: ReviewStatus;
  unresolved_thread_count: number;
}

export type UpdateStatus =
  | { state: "idle" }
  | { state: "checking" }
  | { state: "available"; version: string }
  | { state: "downloading"; progress: number }
  | { state: "ready" }
  | { state: "up-to-date" };

export interface SessionPrEntry {
  pr_url: string;
  selected_file: string | null;
  sidebar_view: SidebarView | null;
  selected_comment_file: string | null;
}

export interface SessionState {
  open_prs: SessionPrEntry[];
  active_pr: string | null;
}

// ---- Mini-player: PR activity widget ----

/** A saved GitHub search that surfaces PRs into the activity feed. */
export interface Watch {
  id: string;
  label: string;
  query: string;
}

/** Observable PR state used for diffing; camelCase mirrors Rust's `Observed`. */
export interface Observed {
  updated_at: string;
  review_state?: string | null;
  unresolved_threads?: number | null;
  head_sha?: string | null;
  comment_count?: number | null;
  ci_state?: string | null;
}

/** A row in the activity feed (matches Rust `PrActivityItem`, camelCase wire). */
export interface PrActivityItem {
  prUrl: string;
  owner: string;
  repo: string;
  number: number;
  title: string;
  author: string;
  avatarUrl: string;
  updatedAt: string;
  draft: boolean;
  reasons: string[];
  deltas: string[];
  reviewState?: string | null;
  unresolvedThreads?: number | null;
  ciState?: string | null;
  unread: boolean;
}

/** Payload of the `pr-activity` event (matches Rust `PrActivityPayload`). */
export interface PrActivityPayload {
  items: PrActivityItem[];
  truncated: Record<string, number>;
  fetchedAt: string;
}
