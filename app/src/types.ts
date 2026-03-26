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

export interface ReviewComment {
  id: string;
  body: string;
  author: CommentAuthor;
  created_at: string;
  updated_at: string;
  url: string;
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

export interface Tab {
  id: string;
  manifest: ReviewManifest;
  selectedFile: FileDiff | null;
  viewedFiles: Set<string>;
  staleViewedFiles: Set<string>;
  commentThreads: CommentThreadsState;
  selectedCommentFile: string | null;
  sidebarView: SidebarView;
  isRefreshing?: boolean;
  lastCommentCount?: number;
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
  filter_older: boolean;
  filter_team: boolean;
}

export type ReviewStatus = "approved" | "changes_requested" | "commented" | "dismissed" | "pending";

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
