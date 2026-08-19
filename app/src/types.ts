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

/** One of the 2-3 highest-risk changes, surfaced in the top-of-review triage card. */
export interface TopRisk {
  /** Short headline (e.g. "Admin-only check removed on /users"). */
  title: string;
  /** One sentence on why it carries risk. */
  detail: string;
  /** File the reviewer should jump to. */
  path: string;
  /** Line (in the head version) to scroll to, when known. */
  start_line?: number | null;
}

/** One file in the contract-first "fastest path" ordering, with a one-line
 * rationale ("defines the shape the rest consumes"). */
export interface ReviewOrderItem {
  path: string;
  rationale: string;
}

/** Triage guidance for large PRs: what to review first and in what order.
 * Absent for small PRs (see the gate in `fetch`). */
export interface TriageReport {
  top_risks: TopRisk[];
  review_order: ReviewOrderItem[];
}

export type RequirementStatus = "covered" | "partial" | "uncovered" | "untestable";

/** A test file backing (or, for an orphan, not backing) a requirement. */
export interface TestRef {
  path: string;
  note?: string | null;
}

/** One requirement extracted from the PR body/title, judged against the PR's
 * test-file diffs. */
export interface RequirementEntry {
  text: string;
  status: RequirementStatus;
  tests: TestRef[];
  note?: string | null;
}

/** Requirements-coverage analysis (issue #179). `null` when the PR body states
 * no real requirements, the gate isn't met, or the AI pass fails to parse. */
export interface RequirementsCoverage {
  requirements: RequirementEntry[];
  orphan_tests: TestRef[];
}

// ── Attention digest (issue #180) ───────────────────────────────────────────
// TS-only for now: entries are derived in the frontend from the manifest and
// checks status (see components/digest.ts). A Rust counterpart arrives when a
// backend pass emits entries directly (#179 adds a coverage source).

export type DigestSeverity = "critical" | "high" | "medium" | "info";

export type DigestJump =
  | { kind: "file"; path: string; line?: number | null }
  | { kind: "checks" }
  | { kind: "url"; url: string }
  | { kind: "requirements" }
  | { kind: "none" };

export interface DigestEntry {
  /** Stable key, e.g. "ci:<check name>" / "risk:<index>". */
  id: string;
  severity: DigestSeverity;
  /** One-line headline. */
  claim: string;
  /** One sentence max, shown muted. */
  detail?: string;
  /** Producing pass. */
  source: "ci" | "triage" | "coverage";
  jump: DigestJump;
  /** Stable text-hash key into the resolved-specs store (see resolved_specs.rs).
   * Present only on coverage entries — CI/triage rows aren't resolvable. */
  resolveKey?: string;
}

/** One commit in a PR's commit list (the "commits" tab). */
export interface PrCommit {
  sha: string;
  message_headline: string;
  author_login: string | null;
  author_avatar: string | null;
  committed_at: string;
}

export interface ReviewManifest {
  pr_title: string;
  pr_url: string;
  pr_number: number;
  base_ref: string;
  head_ref: string;
  base_sha: string;
  head_sha: string;
  author: string;
  draft: boolean;
  summary: string;
  change_groups: ChangeGroup[];
  /** Triage-first guidance (top risks + contract-first order). `null` for small
   * PRs or when the triage AI pass fails without a usable fallback. */
  triage?: TriageReport | null;
  /** Requirements-coverage analysis (issue #179). `null` when the pass didn't
   * run or found nothing to report. */
  requirements_coverage?: RequirementsCoverage | null;
  /** The PR description, truncated char-safely to bound cache size. Empty
   * when the PR has no body or on manifests fetched before this field existed. */
  body: string;
  /** This PR's commits, oldest first. Empty on manifests fetched before this
   * field existed, or when the commits fetch failed (best-effort). */
  commits: PrCommit[];
  files: FileDiff[];
}

/** One file changed by a single commit (the "commit diff" side panel). */
export interface CommitDiffFile {
  path: string;
  status: string;
  additions: number;
  deletions: number;
  /** Absent for large or binary files GitHub doesn't return a patch for. */
  patch: string | null;
  /** The prior path, when this file was renamed. */
  previous_path: string | null;
}

/** The diff for a single commit, fetched on demand when a commit is opened. */
export interface CommitDiff {
  sha: string;
  message_headline: string;
  files: CommitDiffFile[];
  /** True when GitHub's 300-file cap on this endpoint truncated the list. */
  truncated: boolean;
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

export type CheckAnnotationsState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "loaded"; failures: CheckFailures }
  | { status: "error"; message: string };

/** One turn of the per-PR review chat. `filePath` records which file was in
 * focus when a user message was sent (for display); undefined for whole-PR scope. */
export interface ChatMessage {
  role: "user" | "assistant";
  content: string;
  filePath?: string;
}

/** Tab-scoped state for the conversational diff Q&A panel. */
export interface ChatState {
  messages: ChatMessage[];
  /** "idle" between turns; "streaming" while an answer is being generated. */
  status: "idle" | "streaming";
  /** The in-progress assistant answer accumulated from stream deltas. */
  streamingText: string;
  /** Transient status shown while the AI is working but not emitting text
   * (e.g. the CLI agent using tools between blocks); null when none. */
  streamingStatus: string | null;
  /** When true, ground answers in the whole PR rather than the selected file. */
  includeWholePr: boolean;
  /** Whether the chat dock is open. */
  open: boolean;
  /** Last error message from a failed request, if any. */
  error?: string;
}

/** Streaming events sent from the backend `chat_send` command over the IPC channel. */
export type ChatStreamEvent =
  | { type: "delta"; text: string }
  | { type: "status"; label: string | null }
  | { type: "done"; content: string }
  | { type: "error"; message: string };

/** A view-control action the chat model can emit as a fenced ```marrow-action
 * JSON block (see `CHAT_UI_ACTIONS` in `crates/core/src/chat.rs`, the single
 * source of truth for the protocol). Mirrors the schemas documented there —
 * keep the two in sync by hand. */
// Kept in sync BY HAND with CHAT_UI_ACTIONS (crates/core/src/chat.rs) and
// isChatAction (components/RichText.tsx) — edit all three together.
export type ChatAction =
  | { action: "open_file"; path: string; line?: number }
  | { action: "open_overview" }
  | { action: "next_file" }
  | { action: "prev_file" }
  | { action: "open_commit"; sha: string }
  | { action: "set_hunk_filter"; filter: "all" | "high" | "medium" }
  | { action: "set_view_mode"; mode: DiffViewMode }
  | { action: "show_comments"; open: boolean }
  // Manual-only: never auto-executed — clicking the chip opens the inline
  // comment composer prefilled with `body`; the user edits and posts it.
  // `line` anchors the comment (last line, head side); optional `start_line`
  // (< line) makes it a multi-line range.
  | { action: "draft_comment"; path: string; line: number; start_line?: number; body: string };

/** A structured answer card the chat model can emit as a fenced ```marrow-card
 * JSON block (see `CHAT_ANSWER_CARDS` in `crates/core/src/chat.rs`, the single
 * source of truth for the protocol). Mirrors the schemas documented there —
 * keep the two in sync by hand. Pure rendering — no execution, unlike
 * `ChatAction`. */
// Kept in sync BY HAND with CHAT_ANSWER_CARDS (crates/core/src/chat.rs) and
// isChatCard (components/ChatCards.tsx) — edit all three together.
export type ChatCardCell = string | { text: string; path?: string; line?: number };

export interface ChatCardListItem {
  text: string;
  detail?: string;
  path?: string;
  line?: number;
}

export type ChatCard =
  | { type: "table"; title?: string; columns: string[]; rows: ChatCardCell[][] }
  | { type: "list"; title?: string; items: ChatCardListItem[] };

/** A read-only repo tool call the chat model can emit as a fenced
 * ```marrow-tool JSON block (see `CHAT_REPO_TOOLS` in `crates/core/src/chat.rs`,
 * the single source of truth for the protocol). Mirrors the schemas
 * documented there — keep the two in sync by hand. Execution is
 * backend-only (chat_agent.rs) — the frontend only renders the fence as a
 * chip, unlike `ChatAction`. */
// Kept in sync BY HAND with CHAT_REPO_TOOLS (crates/core/src/chat.rs) and
// isChatToolCall (components/RichText.tsx) — edit all three together.
export type ChatToolCall =
  | { tool: "read_file"; path: string }
  | { tool: "search_code"; query: string }
  | { tool: "list_dir"; path: string };

export type NoteResolutionState = "fixed" | "intentional" | "noise";

/** How/why an AI note was resolved. Mirrors `NoteResolution` in
 * `dismissed_highlights.rs` — `reason`/`at` are optional there too (empty
 * string on the wire), kept optional here for the same reason. */
export interface NoteResolution {
  /** Normally a NoteResolutionState, but Rust serde-defaults a malformed
   * entry's state to "" rather than failing the whole file, so loaded values
   * can fall outside the union (rendered as plain "Dismissed"). The
   * `string & {}` keeps union autocomplete while admitting that. */
  state: NoteResolutionState | (string & {});
  reason?: string;
  at?: string;
}

export type SidebarView = "groups" | "category" | "tree";

/** Which of the PR view's persistent lenses is showing (issue #170; Checks
 * added in issue #175). */
export type PrLens = "overview" | "files" | "commits" | "checks";

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
  /** last PR ref this tab tried to fetch — lets a failed fetch offer Retry */
  lastPrRef?: string | null;
  selectedFile: FileDiff | null;
  /** Which of Overview/Files/Commits this tab is showing — persistent per tab
   * (issue #170), not a global mode. */
  lens: PrLens;
  /** The commit shown in the Commits lens, if any. Per-tab so switching tabs
   * can't leak one PR's commit scope onto another's canvas. */
  selectedCommit: PrCommit | null;
  /** Change-group name scoping the Files sidebar (the "Group: <name> ✕" pill),
   * from a change-group deep link. Null = no filter. */
  groupFilter: string | null;
  viewedFiles: Set<string>;
  staleViewedFiles: Set<string>;
  /** Keys (see highlightKey) of AI highlights the user has dismissed for this PR. */
  dismissedHighlights: Set<string>;
  /** Resolution metadata (state + reason) for dismissed highlights, keyed by
   * highlightKey. A dismissed key may have no entry here (plain/legacy dismiss). */
  noteResolutions: Map<string, NoteResolution>;
  /** Keys (see DigestEntry.resolveKey) of coverage-digest spec items the user
   * has marked addressed for this PR. Resolution never means "covered" —
   * it's a user acknowledgment, orthogonal to the AI's coverage judgment. */
  resolvedSpecKeys: Set<string>;
  /** Resolution metadata (state + reason) for resolved spec items, keyed by
   * resolveKey. Mirrors noteResolutions' shape/lifecycle. */
  specResolutions: Map<string, NoteResolution>;
  /** User-provided requirements text (issue #179 phase 2), saved locally and
   * fed into the next coverage pass as the authoritative extraction source.
   * `null` when nothing's been saved. */
  localRequirements: string | null;
  /** True while the coverage-only re-analysis triggered by saving
   * requirements is in flight. */
  analyzingRequirements: boolean;
  /** Keys (see highlightKey) of AI highlights newly introduced by the most
   * recent refresh's re-analysis, relative to the manifest it replaced.
   * Transient — not persisted, and undefined outside a just-refreshed tab. */
  newHighlightKeys?: Set<string>;
  /** Conversational diff Q&A state for this PR. */
  chat: ChatState;
  commentThreads: CommentThreadsState;
  /** Inline CI failure annotations for this tab's PR, fetched on demand once
   * a failing check run is observed (see the fetch effect in App.tsx). */
  checkAnnotations: CheckAnnotationsState;
  /** Whether the right-dock comments panel is open (mutually exclusive with `chat.open`). */
  commentsOpen?: boolean;
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
  merged: boolean;
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
  /** Whether the review queue shows draft PRs. On by default (current
   * behavior); the frontend filters draft rows out when this is off. */
  show_draft_prs: boolean;
  setup_done: boolean;
  /** When true, files open with every hunk expanded instead of auto-collapsing
   * low-significance hunks (issue #55). Off by default to keep the
   * collapsed-by-default behavior. */
  expand_all_hunks: boolean;
}

export type ReviewStatus = "approved" | "changes_requested" | "commented" | "dismissed" | "pending";

export interface MyReviewState {
  status: ReviewStatus;
  is_re_requested: boolean;
  is_merged: boolean;
  author: string;
  draft: boolean;
  approved_by: string[];
  /** Lowercased GitHub `mergeable` enum: "mergeable" | "conflicting" |
   * "unknown". Empty when GitHub hasn't computed it or the field is absent. */
  mergeable: string;
  labels: PrLabel[];
  /** The SHA the viewer's most recent review was submitted against, and when.
   * `null` if the viewer hasn't reviewed this PR. */
  last_reviewed_sha: string | null;
  last_reviewed_at: string | null;
}

export interface PrLabel {
  name: string;
  /** Hex color without the leading '#', as GitHub returns it. */
  color: string;
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

/** One inline annotation on a failing check run (a lint/test failure pinned
 * to a file + line range on the head commit). */
export interface CheckAnnotation {
  path: string;
  start_line: number;
  end_line: number;
  annotation_level: string;
  message: string;
  title: string | null;
  check_name: string;
}

/** Annotations from a PR's failing checks, fetched on demand for the head SHA. */
export interface CheckFailures {
  head_sha: string;
  annotations: CheckAnnotation[];
  truncated: boolean;
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
  approval_count: number;
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
  /** Mirrors Rust's `Option<String>` — hand-validated against `PrLens` on
   * restore rather than typed as `PrLens | null` here (an old session file, or
   * a future value, could carry anything). */
  lens?: string | null;
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
  /** "needs_you" | "yours" | "watching" — see Rust `compute_tier`. */
  tier: string;
  /** Sortable relevance score — see Rust `compute_urgency`. Not used by the
   * current widget; for a future queue view to sort by. */
  urgency: number;
  /** Muted until the next delta wakes it. Current widget ignores this. */
  snoozed: boolean;
}

/** Payload of the `pr-activity` event (matches Rust `PrActivityPayload`). */
export interface PrActivityPayload {
  items: PrActivityItem[];
  truncated: Record<string, number>;
  fetchedAt: string;
}
