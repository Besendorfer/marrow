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
