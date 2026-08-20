//! Centralized character budgets for every AI-pass prompt builder (issue #191).
//!
//! Sizing rationale: the models we target have ~200k-token context windows,
//! and a token averages ~3-4 chars of code/diff text — call it 600k+ chars of
//! usable context. Each pass's worst-case assembled prompt is kept under
//! ~300k chars, leaving comfortable headroom for instructions, the response,
//! and token-per-char variance. Budgets are chars (not tokens) because every
//! builder truncates with char-safe `truncate_chars`-style helpers.
//!
//! When a builder does cut input at one of these budgets, it reports it (the
//! `(String, bool)` builder returns) and the run's manifest sets
//! `analysis_truncated` so the residual giant-PR case is never silent.

/// Classification pass: the whole PR diff in one prompt.
/// Worst case: 200k diff + file list + instructions ≈ 210k chars.
pub const CLASSIFICATION_DIFF: usize = 200_000;

/// Highlight pass, per-file diff cap.
pub const HIGHLIGHT_PER_FILE: usize = 25_000;
/// Highlight pass, shared pool across all relevant files' diffs.
/// Worst case: 250k diffs + 10k body + prior notes + instructions ≈ 265k chars.
pub const HIGHLIGHT_TOTAL: usize = 250_000;
/// Highlight pass, PR description cap.
pub const HIGHLIGHT_BODY: usize = 10_000;

/// Triage pass, per-file diff cap (ordering needs signatures/imports, not
/// full bodies — still tighter than the highlight pass).
pub const TRIAGE_PER_FILE: usize = 10_000;
/// Triage pass, shared diff pool.
/// Worst case: 100k diffs + file info + instructions ≈ 110k chars.
pub const TRIAGE_TOTAL: usize = 100_000;

/// Coverage pass, per-file cap for changed test / inline-test diffs.
pub const COVERAGE_PER_FILE: usize = 10_000;
/// Coverage pass, shared pool for changed test + inline-test diffs.
pub const COVERAGE_TOTAL: usize = 100_000;
/// Coverage pass, per-file cap for existing (unchanged) test files.
pub const COVERAGE_EXISTING_PER_FILE: usize = 10_000;
/// Coverage pass, shared pool for existing test files.
pub const COVERAGE_EXISTING_TOTAL: usize = 40_000;
/// Coverage pass, per-issue cap for linked-issue text. Reserved: no builder
/// consumes linked issues yet — sized here so that section lands pre-budgeted.
pub const COVERAGE_ISSUE_PER_ISSUE: usize = 5_000;
/// Coverage pass, shared pool for linked-issue text (reserved, see above).
pub const COVERAGE_ISSUE_TOTAL: usize = 20_000;
/// Coverage pass, PR description cap.
pub const COVERAGE_BODY: usize = 20_000;
/// Coverage pass, user-provided requirements cap (same discipline as the body).
/// Coverage worst case: 100k + 40k + 20k + 20k (+20k reserved) + changed-path
/// list + instructions ≈ 210k chars.
pub const COVERAGE_USER_REQS: usize = 20_000;

/// Chat grounding, per-file diff cap.
pub const CHAT_PER_FILE_DIFF: usize = 15_000;
/// Chat grounding, per-file full-content cap (when head content is included).
pub const CHAT_PER_FILE_CONTENT: usize = 20_000;
/// Chat grounding, overall ceiling on the assembled file context.
/// Worst case: 120k context + protocol/instruction text (~7k) ≈ 127k chars,
/// leaving room for the conversation itself on top.
pub const CHAT_TOTAL_CONTEXT: usize = 120_000;
