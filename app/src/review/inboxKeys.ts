// The inbox review list's keyboard map (issue #238 phase 4), kept pure so
// its guards are testable without a DOM. ReviewInbox applies the result.

import type { FindingState } from "./findings";

export type ListKeyAction =
  | { type: "move"; delta: 1 | -1 }
  | { type: "fine" }
  | { type: "dismiss" }
  | { type: "comment" };

/** What a plain keypress in the review list does, given the selected
 * finding's state (null when the selection isn't a finding). null = not
 * ours; let it through. */
export function listKeyAction(key: string, finding: { state: FindingState } | null): ListKeyAction | null {
  switch (key) {
    case "j":
    case "ArrowDown":
      return { type: "move", delta: 1 };
    case "k":
    case "ArrowUp":
      return { type: "move", delta: -1 };
    case "e":
      // Nothing to mark on an already-handled finding.
      return finding && (finding.state === "open" || finding.state === "commented") ? { type: "fine" } : null;
    case "x":
      return finding && finding.state !== "dismissed" ? { type: "dismiss" } : null;
    case "c":
      return finding ? { type: "comment" } : null;
    default:
      return null;
  }
}

/** Where the list goes after acting on `fromId`: the next OPEN finding after
 * it in list order (wrapping; the acted-on one is skipped — its new state
 * hasn't committed yet), else the next list item, else nowhere. */
export function nextAfterAction(
  findings: { id: string; state: FindingState }[],
  navIds: string[],
  fromId: string,
): string | null {
  const idx = findings.findIndex((f) => f.id === fromId);
  for (let step = 1; step < findings.length; step++) {
    const f = findings[(idx + step) % findings.length];
    if (f.state === "open" && f.id !== fromId) return f.id;
  }
  const pos = navIds.indexOf(fromId);
  return pos >= 0 && pos + 1 < navIds.length ? navIds[pos + 1] : null;
}

export type ChooserKeyAction = { type: "pick"; index: number } | { type: "cancel" };

/** Keys while the "Not an issue" reason picker is open: 1–n pick a reason,
 * x again (or Enter) picks the first ("Not a real issue") so `x x` stays a
 * two-key quick dismiss, Escape cancels. null = not a picker key. */
export function chooserKeyAction(key: string, optionCount: number): ChooserKeyAction | null {
  if (key === "Escape") return { type: "cancel" };
  if (key === "x" || key === "Enter") return { type: "pick", index: 0 };
  const n = Number(key);
  return Number.isInteger(n) && n >= 1 && n <= optionCount ? { type: "pick", index: n - 1 } : null;
}
