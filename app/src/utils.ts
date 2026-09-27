import type { CheckRunInfo, ContextRead, Highlight, PrChecksStatus } from "./types";

export function getFileName(path: string): string {
  return path.split("/").pop() || path;
}

/** Whether a check run counts as failing. GitHub conclusions arrive
 * uppercase (see the CiChip comment in PrOverview.tsx). Mirrors Rust's
 * `failing_conclusion` (github.rs) — failure/timed_out/action_required,
 * deliberately NOT cancelled (a user-initiated stop's partial annotations
 * are noise) — so a run this badges as failing always has annotations to
 * show, never a "N ✗" pointing at an empty Checks lens. Single source of
 * truth reused by the overview CI chip, the header lens badge, and the
 * Checks lens (issue #175). */
export function isFailingCheck(check: CheckRunInfo): boolean {
  return (
    check.conclusion === "FAILURE" ||
    check.conclusion === "TIMED_OUT" ||
    check.conclusion === "ACTION_REQUIRED"
  );
}

export function countFailingChecks(checks: PrChecksStatus): number {
  return checks.check_runs.filter(isFailingCheck).length;
}

// djb2 string hash → unsigned base36, for compact stable keys.
export function hashString(s: string): string {
  let h = 5381;
  for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) | 0;
  return (h >>> 0).toString(36);
}

/**
 * Stable identifier for an AI highlight, used to persist dismissals. Includes the
 * comment text (hashed) so a dismissal survives re-fetches but re-surfaces if the
 * note's wording changes (i.e. the AI flagged something different).
 */
export function highlightKey(
  path: string,
  h: { start_line: number; end_line: number; comment: string },
): string {
  return `${path}:${h.start_line}-${h.end_line}:${hashString(h.comment)}`;
}

export function parsePrUrl(url: string): { owner: string; repo: string; number: number } {
  const match = url.match(/github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)/);
  if (!match) throw new Error(`Invalid PR URL: ${url}`);
  return { owner: match[1], repo: match[2], number: parseInt(match[3], 10) };
}

export function extractPrRef(input: string): string | null {
  // Mirrors the tightened backend regex in pr_parser.rs so the frontend
  // doesn't accept refs the backend would reject.
  const match = input.match(
    /github\.com\/([A-Za-z0-9][A-Za-z0-9-]{0,38}\/[A-Za-z0-9._-]{1,100}\/pull\/\d+)/
  );
  return match ? match[1] : null;
}

/**
 * Normalize a GitHub PR URL or an `owner/repo#number` ref to a single
 * comparable key (`owner/repo#number`, lowercased), or null if neither shape
 * matches. Used to tell whether two references point at the same PR.
 */
export function canonicalPrKey(input: string): string | null {
  let owner: string | undefined, repo: string | undefined, number: string | undefined;
  const urlMatch = input.match(/github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)/);
  if (urlMatch) {
    [, owner, repo, number] = urlMatch;
  } else {
    const refMatch = input.match(/^([^/]+)\/([^/#]+)#(\d+)$/);
    if (!refMatch) return null;
    [, owner, repo, number] = refMatch;
  }
  return `${owner}/${repo}#${number}`.toLowerCase();
}

/** Relative time. `short` drops the " ago" suffix (for compact chips). */
export function timeAgo(dateStr: string, short = false): string {
  const then = new Date(dateStr).getTime();
  if (Number.isNaN(then)) return "";
  const seconds = Math.floor((Date.now() - then) / 1000);
  if (seconds < 60) return "just now";
  const suffix = short ? "" : " ago";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m${suffix}`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h${suffix}`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d${suffix}`;
  const months = Math.floor(days / 30);
  return `${months}mo${suffix}`;
}

/** Draft body for posting an AI note as a review comment (issue #231): the
 * note, then its scenario and fix — no severity tag, so it reads as the
 * reviewer's own words. */
export function highlightCommentBody(h: Highlight): string {
  const parts = [h.comment];
  if (h.scenario) parts.push(h.scenario);
  if (h.fix) parts.push(`Suggested fix: ${h.fix}`);
  return parts.join("\n\n");
}

/** One "Context used" row (issue #232): what the review read or searched,
 * with the repo shown only when it isn't this PR's. */
export function contextRow(r: ContextRead, prRepo: string): string {
  const other = r.repo !== prRepo && !r.repo.endsWith("/*");
  const where = other ? `${r.repo} · ` : "";
  switch (r.tool) {
    case "search_code":
      return r.rev === "owner" ? `Searched all ${r.repo.replace("/*", "")} repos for “${r.path}”` : `Searched for “${r.path}”`;
    case "list_dir":
      return `Listed ${where}${r.path || "repo root"}`;
    default:
      return `Read ${where}${r.path}${r.rev === "base" ? " (before this PR)" : ""}`;
  }
}

/** `owner/name` of the PR, from its URL. */
export function prRepoOf(url: string): string {
  const m = url.match(/github\.com\/([^/]+\/[^/]+)\/pull\//);
  return m ? m[1] : "";
}

