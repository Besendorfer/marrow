// Commits handlers (moved verbatim from App.tsx in issue #238 phase 2).
// Calls into other modules go through `ctx`, which holds every handler of
// the current render — the same per-render closure semantics as before.

import { invoke } from "@tauri-apps/api/core";
import type { PrCommit, CommitDiff } from "../types";
import type { ReviewCtx } from "./ctx";

// `ctxArg` is typed unknown only so ReturnType<typeof create…> (which
// ReviewCtx is built from) doesn't loop through this parameter's type.
export function createCommits(ctxArg: unknown) {
  const ctx = ctxArg as ReviewCtx;
  const {
    activeTabId,
    setCommitDiffLoading,
    setCommitDiffError,
    setCommitDiff,
    commitDiffCacheRef,
    commitDiffFetchingRef,
    tabsRef,
    activeTabIdRef,
  } = ctx;

  /** Load `commit`'s diff for `tabId` into the shared App-level slots —
   * serving the cache when it hits, and fetching on a miss. The single fetch
   * path for both an interactive commit click (handleViewCommit) and the
   * tab-switch resync effect above, so they can't diverge. Only ever writes
   * the *visible* slots (setCommitDiff/Loading/Error) when `tabId` is still
   * the active tab at the time — cache writes are unconditional (shas are
   * immutable, so a background-tab fetch resolving late is still good data
   * for whenever that tab becomes active again). `commitDiffFetchingRef`
   * dedupes a request already in flight for this exact tab+sha (the resync
   * effect and a click can both ask for the same thing in the same tick). */
  async function loadCommitDiff(tabId: string, commit: PrCommit) {
    const key = `${tabId}:${commit.sha}`;
    const cached = commitDiffCacheRef.current.get(commit.sha);
    if (cached) {
      if (activeTabIdRef.current === tabId) {
        setCommitDiff(cached);
        setCommitDiffError(null);
        setCommitDiffLoading(false);
      }
      return;
    }
    if (activeTabIdRef.current === tabId) {
      setCommitDiff(null);
      setCommitDiffError(null);
      setCommitDiffLoading(true);
    }
    if (commitDiffFetchingRef.current.has(key)) return;
    const tab = tabsRef.current.find((t) => t.id === tabId);
    if (!tab?.manifest) return;
    commitDiffFetchingRef.current.add(key);
    try {
      const diff = await invoke<CommitDiff>("get_commit_diff", {
        prRef: tab.manifest.pr_url,
        sha: commit.sha,
      });
      commitDiffCacheRef.current.set(commit.sha, diff);
      // A late resolve after the user switched to another commit, or away
      // from this tab entirely, must not clobber whatever's now showing.
      const current = tabsRef.current.find((t) => t.id === tabId);
      if (activeTabIdRef.current === tabId && current?.selectedCommit?.sha === commit.sha) {
        setCommitDiff(diff);
        setCommitDiffLoading(false);
      }
    } catch (err) {
      const current = tabsRef.current.find((t) => t.id === tabId);
      if (activeTabIdRef.current === tabId && current?.selectedCommit?.sha === commit.sha) {
        setCommitDiffError(String(err));
        setCommitDiffLoading(false);
      }
    } finally {
      commitDiffFetchingRef.current.delete(key);
    }
  }

  /** Commit row click (Commits card, Commits lens rail, or a Newer/Older nav)
   * — enters/moves commit scope on `tabId` (defaulting to the active tab) and
   * switches it to the Commits lens. Per-tab as of #170: `selectedCommit`
   * lives on the tab so it can't leak onto another tab's canvas; the fetched
   * diff itself stays a single App-level slot (see loadCommitDiff above). */
  function handleViewCommit(commit: PrCommit, tabId: string = activeTabId!) {
    ctx.updateTab(tabId, (t) => ({ ...t, selectedCommit: commit, lens: "commits" }));
    loadCommitDiff(tabId, commit);
  }

  return { loadCommitDiff, handleViewCommit };
}

export type CommitsApi = ReturnType<typeof createCommits>;
