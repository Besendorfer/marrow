// The Finish panel (issue #238 phase 5): one per-tab place to wrap up a
// review — recap, batched comments, CI as a status line, verdict, body,
// submit — replacing the header submit dropdown, the global keyboard review
// picker, and the CI blocking modal. Pure rules first (tested), then the
// handlers the panel calls.

import { invoke } from "@tauri-apps/api/core";
import type { FinishDone, FinishDraft, PrChecksStatus, ReviewManifest, ReviewRequestItem, ReviewThread } from "../types";
import { canonicalPrKey, isFailingCheck } from "../utils";
import type { ReviewCtx } from "./ctx";

export type { ReviewEvent } from "../types";
import type { ReviewEvent } from "../types";

/** Your comments waiting in GitHub's pending review — posted from the diff,
 * visible only to you until the review is submitted. Pending comments are by
 * definition the viewer's own. */
export function pendingComments(threads: ReviewThread[] | undefined): { thread: ReviewThread; body: string }[] {
  const out: { thread: ReviewThread; body: string }[] = [];
  for (const thread of threads ?? []) {
    for (const c of thread.comments) {
      if (c.pending) out.push({ thread, body: c.body });
    }
  }
  return out;
}

/** Why a review can't be submitted yet, or null when it can. */
export function submitBlocker(event: ReviewEvent, body: string, pendingCount: number, isMerged: boolean): string | null {
  if (isMerged && event !== "COMMENT") return "This PR is merged, so only a comment review is possible.";
  if (event === "REQUEST_CHANGES" && !body.trim()) return "Say what needs to change.";
  if (event === "COMMENT" && !body.trim() && pendingCount === 0) {
    return "Write a comment, or add at least one comment on the diff.";
  }
  return null;
}

/** CI as a status line — information, never a blocker. */
export function ciStatus(checks: PrChecksStatus | null | undefined): { tone: "ok" | "running" | "fail" | "none"; text: string } {
  const runs = checks?.check_runs ?? [];
  if (runs.length === 0) return { tone: "none", text: "No CI checks reported" };
  const failing = runs.filter(isFailingCheck);
  if (failing.length > 0) {
    return { tone: "fail", text: failing.length === 1 ? `CI: ${failing[0].name} is failing` : `CI: ${failing.length} checks failing` };
  }
  if (runs.some((r) => r.status !== "COMPLETED")) return { tone: "running", text: "CI is still running" };
  return { tone: "ok", text: "CI passing" };
}

/** An AI draft never replaces text the reviewer has already typed. */
export function mergeDraft(current: string, drafted: string): string {
  return current.trim() ? current : drafted;
}

/** The verdict preselected until the reviewer picks one: a comment on a merged
 * PR, request changes while claimed defects are open, else approve. Derived
 * every render, so it settles once threads load (a finding you commented on
 * stops counting as open). */
export function defaultVerb(isMerged: boolean, openFixCount: number): ReviewEvent {
  if (isMerged) return "COMMENT";
  return openFixCount > 0 ? "REQUEST_CHANGES" : "APPROVE";
}

/** Is this queue item a good "review next"? It must still be waiting on you
 * (not reviewed yet, or your review was dismissed), not a draft, and not
 * already open in a tab. `fetch_review_requests` also returns PRs you've
 * reviewed or commented on, so this filter matters. */
export function isNextCandidate(item: ReviewRequestItem, openKeys: Set<string | null>): boolean {
  if (item.draft) return false;
  if (item.my_review_status !== "pending" && item.my_review_status !== "dismissed") return false;
  return !openKeys.has(canonicalPrKey(`${item.owner}/${item.repo}#${item.number}`));
}

/** Relevant files you've marked reviewed, out of all relevant files. */
export function filesReviewed(manifest: ReviewManifest, viewed: Set<string>): { reviewed: number; total: number } {
  const relevant = manifest.files.filter((f) => f.classification !== "NOT_RELEVANT");
  return { reviewed: relevant.filter((f) => viewed.has(f.path)).length, total: relevant.length };
}

// `ctxArg` is typed unknown only so ReturnType<typeof create…> (which
// ReviewCtx is built from) doesn't loop through this parameter's type.
export function createFinish(ctxArg: unknown) {
  const ctx = ctxArg as ReviewCtx;
  const { activeTabId, tabsRef } = ctx;

  function openFinish() {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab?.manifest) return;
    ctx.updateTab(tab.id, (t) => ({ ...t, finishOpen: true }));
  }

  /** Closing keeps an unsent draft for next time; after a submit, the next
   * open starts fresh. */
  function closeFinish() {
    ctx.updateTab(activeTabId, (t) =>
      t.finishDone ? { ...t, finishOpen: false, finishDone: null, finishDraft: null } : { ...t, finishOpen: false },
    );
  }

  /** The panel's draft (body, chosen verdict) lives on the tab, so switching
   * tabs and back doesn't lose what you typed or re-run the AI draft. */
  function setFinishDraft(patch: Partial<FinishDraft>) {
    ctx.updateTab(activeTabId, (t) => ({ ...t, finishDraft: { ...t.finishDraft, ...patch } }));
  }

  function setFinishDone(done: FinishDone) {
    ctx.updateTab(activeTabId, (t) => ({ ...t, finishDone: done }));
  }

  /** Fetch fresh threads (so the recap's pending and unresolved counts are
   * current), store them on the tab, and ask the AI for a review body that
   * reflects the unresolved ones. */
  async function draftReviewBody(): Promise<string> {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab?.manifest) return "";
    const tabId = tab.id;
    const threads = await invoke<ReviewThread[]>("fetch_review_comments", { prUrl: tab.manifest.pr_url });
    ctx.updateTab(tabId, (t) => ({ ...t, commentThreads: { status: "loaded", threads } }));
    const unresolved = threads.filter((t) => !t.is_resolved);
    const threadsJson = JSON.stringify(
      unresolved.map((t) => ({
        path: t.path,
        line: t.line,
        comments: t.comments.map((c) => ({ author: c.author.login, body: c.body })),
      })),
    );
    return invoke<string>("generate_review_body", {
      threadsJson,
      prTitle: tab.manifest.pr_title,
      hasUnresolved: unresolved.length > 0,
    });
  }

  /** The next PR waiting on your review that isn't already open in a tab. */
  async function nextInQueue(): Promise<ReviewRequestItem | null> {
    const d = new Date();
    d.setDate(d.getDate() - 30);
    const items = await invoke<ReviewRequestItem[]>("fetch_review_requests", {
      cutoffDate: d.toISOString().split("T")[0],
      fetchRecent: true,
    });
    const open = new Set(
      tabsRef.current.map((t) => (t.manifest ? canonicalPrKey(t.manifest.pr_url) : null)).filter(Boolean),
    );
    return items.find((i) => isNextCandidate(i, open)) ?? null;
  }

  return { openFinish, closeFinish, setFinishDraft, setFinishDone, draftReviewBody, nextInQueue };
}

export type FinishApi = ReturnType<typeof createFinish>;
