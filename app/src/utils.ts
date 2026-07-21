export function getFileName(path: string): string {
  return path.split("/").pop() || path;
}

// djb2 string hash → unsigned base36, for compact stable keys.
function hashString(s: string): string {
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
 * True when the input is something `fetch_pr` can open: a PR URL, an
 * `owner/repo#123` or `owner/repo/pull/123` ref, or a bare `#123` (resolved
 * against the configured default repo by the backend). Mirrors the accepted
 * shapes in pr_parser.rs — the omnibox uses this to decide "open" vs "filter".
 */
export function isOpenablePrRef(input: string): boolean {
  if (extractPrRef(input) !== null) return true;
  return (
    /^[A-Za-z0-9][A-Za-z0-9-]{0,38}\/[A-Za-z0-9._-]{1,100}(#\d+|\/pull\/\d+)$/.test(input) ||
    /^#\d+$/.test(input)
  );
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
