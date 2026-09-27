// Module-level helpers shared by the review controller and App (moved
// verbatim from App.tsx in issue #238 phase 2).

import type { ReviewManifest, Tab, ChatState } from "../types";
import { highlightKey } from "../utils";

/** An empty "open a PR" tab — no loaded PR, not mid-fetch, no error. */
export function isOpenerTab(tab: Tab): boolean {
  return !tab.manifest && !tab.loading && !tab.error;
}

/** Prompt sent by the "Brief me" command — a whole-PR guided walkthrough.
 * Deliberately strict about brevity: without hard limits the model produces an
 * exhausting per-change essay instead of a scannable briefing. */
export const BRIEF_ME_PROMPT =
  "Brief me on this PR — a briefing I can scan in under a minute, not an essay. " +
  "Start with a one-sentence TL;DR. Then one line per change, most important first: " +
  "a `file:line` citation (in backticks, so I can jump there), what it does, and its sharpest risk if it has one. " +
  "Hard limits: at most 7 lines, roughly 25 words each, no sub-bullets, no headings, no code snippets, no restating the diff. " +
  "Merge related changes into one line. Skip filler like 'low risk' or 'mechanical change' — silence means fine. " +
  "End with one line: where to spend my review time. If I want depth on a stop, I'll ask.";

/** Ceiling on marrow-action blocks auto-executed per streaming turn — the
 * backstop behind the prompt's "at most a few actions per reply". */
export const MAX_AUTO_ACTIONS_PER_TURN = 6;

/** A fresh, closed chat panel for a new tab. */
export function emptyChatState(): ChatState {
  return { messages: [], status: "idle", streamingText: "", streamingStatus: null, includeWholePr: false, open: false };
}

/** The repo's base GitHub URL (e.g. `https://github.com/owner/repo`), derived
 * by stripping the `/pull/<n>` suffix off a PR URL — used to build commit URLs. */
export function repoBaseUrl(prUrl: string): string {
  return prUrl.replace(/\/pull\/\d+\/?$/, "");
}

/** Every AI highlight key (see highlightKey) across a manifest's files. */
export function collectHighlightKeys(manifest: ReviewManifest): Set<string> {
  const keys = new Set<string>();
  for (const f of manifest.files) {
    for (const h of f.highlights) keys.add(highlightKey(f.path, h));
  }
  return keys;
}
