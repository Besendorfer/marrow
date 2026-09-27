// Navigation handlers (moved verbatim from App.tsx in issue #238 phase 2).
// Calls into other modules go through `ctx`, which holds every handler of
// the current render — the same per-render closure semantics as before.

import { invoke } from "@tauri-apps/api/core";
import type { FileDiff, Tab, SidebarView, PrLens, ChangeGroup } from "../types";
import type { ReviewCtx } from "./ctx";

// `ctxArg` is typed unknown only so ReturnType<typeof create…> (which
// ReviewCtx is built from) doesn't loop through this parameter's type.
export function createNavigation(ctxArg: unknown) {
  const ctx = ctxArg as ReviewCtx;
  const {
    activeTabId,
    diffViewerRef,
    visibleOrderRef,
    addToast,
    activeTab,
    pendingRevealLineRef,
    tabsRef,
  } = ctx;

  // ── Guided review path ──────────────────────────────────────────────────
  // The review order is the sidebar's visible order (falls back to relevant
  // files in manifest order before the sidebar reports in). The sidebar is
  // unmounted while the overview shows, so the ref can hold another tab's
  // paths — anything not in this manifest is dropped before use. Defaults to
  // activeTab (the common case); setLens takes an explicit tab so it works
  // correctly even when called for a tab that isn't (yet) active.
  function guidedOrder(tab: Tab | null = activeTab): string[] {
    if (!tab?.manifest) return [];
    const inManifest = new Set(tab.manifest.files.map((f) => f.path));
    // visibleOrderRef only ever reflects the mounted (active) tab's sidebar.
    const visible = tab.id === activeTabId ? visibleOrderRef.current.filter((p) => inManifest.has(p)) : [];
    const candidates = visible.length
      ? visible
      : tab.manifest.files
          .filter((f) => f.classification !== "NOT_RELEVANT")
          .map((f) => f.path);

    const reviewOrder = tab.manifest.triage?.review_order;
    if (!reviewOrder?.length) return candidates;

    const candidateSet = new Set(candidates);
    const triaged = reviewOrder.map((item) => item.path).filter((p) => candidateSet.has(p));
    const triagedSet = new Set(triaged);
    const remaining = candidates.filter((p) => !triagedSet.has(p));
    return [...triaged, ...remaining];
  }

  // Rationale for a path from the triage review order, if any (null when
  // triage is absent or the path isn't in it).
  function triageRationale(path: string): string | null {
    const reviewOrder = activeTab?.manifest?.triage?.review_order;
    if (!reviewOrder?.length) return null;
    return reviewOrder.find((item) => item.path === path)?.rationale ?? null;
  }

  // First unviewed file after `fromIdx` in review order (wrapping), treating
  // `alsoViewed` as already reviewed — used when the current file was just
  // marked but state hasn't committed yet. Defaults to activeTab; see guidedOrder.
  function nextUnviewed(order: string[], fromIdx: number, alsoViewed?: string, tab: Tab | null = activeTab): FileDiff | null {
    if (!tab?.manifest || order.length === 0) return null;
    const viewed = tab.viewedFiles;
    for (let step = 1; step <= order.length; step++) {
      const p = order[(fromIdx + step + order.length) % order.length];
      if (!viewed.has(p) && p !== alsoViewed) {
        return tab.manifest.files.find((f) => f.path === p) ?? null;
      }
    }
    return null;
  }

  function markReviewedAndAdvance() {
    if (!activeTab?.selectedFile) return;
    const path = activeTab.selectedFile.path;
    const order = guidedOrder();
    if (!activeTab.viewedFiles.has(path)) ctx.toggleViewed(path);
    const next = nextUnviewed(order, order.indexOf(path), path);
    if (next) setSelectedFile(next);
  }

  // ── Keyboard shortcuts (ported from the CLI/TUI; see useKeyboardShortcuts) ──
  // Returns whether it actually navigated, so callers that report outcomes
  // (chat action chips) don't claim success for an edge-of-list no-op.
  function selectAdjacentFile(delta: 1 | -1): boolean {
    if (!activeTab?.manifest || !activeTab.selectedFile) return false;
    const order = visibleOrderRef.current.length
      ? visibleOrderRef.current
      : activeTab.manifest.files.map((f) => f.path);
    const byPath = (p: string) => activeTab.manifest!.files.find((f) => f.path === p);
    const i = order.indexOf(activeTab.selectedFile.path);
    if (i === -1) {
      // Current file is filtered out of the list — jump to the first visible one.
      const first = byPath(order[0]);
      if (first) { setSelectedFile(first); return true; }
      return false;
    }
    const next = byPath(order[Math.min(Math.max(i + delta, 0), order.length - 1)]);
    if (next && next.path !== activeTab.selectedFile.path) { setSelectedFile(next); return true; }
    return false;
  }

  /** Resolve a file mention (exact path, or a unique suffix match) against a
   * manifest's files — shared by handleChatOpenFile and the chat-action
   * dispatcher so both agree on what counts as a resolvable path. */
  function resolveManifestFile(files: FileDiff[], path: string): FileDiff | undefined {
    return (
      files.find((f) => f.path === path) ??
      (() => {
        const suffixMatches = files.filter((f) => f.path.endsWith("/" + path));
        return suffixMatches.length === 1 ? suffixMatches[0] : undefined;
      })()
    );
  }

  /** Resolve a file mention (exact path, or a unique suffix match against the
   * manifest), select it, and reveal the given head line — expanding its hunk
   * if collapsed. Serves chat citations, top-risk rows, and check-failure rows. */
  async function handleChatOpenFile(path: string, line?: number) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab?.manifest) return;
    let target = resolveManifestFile(tab.manifest.files, path);
    if (!target) return;
    if (line != null && !target.head_content && target.diff_type !== "removed") {
      // Jump targets in files whose contents weren't fetched at analysis time
      // (NOT_RELEVANT files): pull the head version on demand so the whole-file
      // fallback in revealLine has something to show, and remember it on the
      // manifest for the rest of the session.
      const tabId = tab.id;
      try {
        const content = await invoke<string>("get_file_content", {
          prRef: tab.manifest.pr_url,
          path: target.path,
          refSha: tab.manifest.head_sha,
        });
        const patched = { ...target, head_content: content };
        ctx.updateTab(tabId, (t) =>
          t.manifest
            ? {
                ...t,
                manifest: {
                  ...t.manifest,
                  files: t.manifest.files.map((f) => (f.path === patched.path ? patched : f)),
                },
              }
            : t
        );
        target = patched;
      } catch {
        // Content unavailable (deleted path, network) — fall through; the
        // reveal will report honestly.
      }
    }
    if (line != null && target.path === tab.selectedFile?.path && target.head_content === tab.selectedFile?.head_content) {
      if (tab.lens === "files") {
        // Already viewing the file — the mounted DiffViewer can jump directly.
        if (diffViewerRef.current?.revealLine(line) === false) notifyLineOutsideDiff(line);
      } else {
        // Right file, wrong lens: this fast path predates the lens switcher
        // and used to reveal into a hidden canvas. Switch to Files and let
        // the pending-reveal effect jump once the viewer mounts.
        pendingRevealLineRef.current = line;
        ctx.updateTab(tab.id, (t) => ({ ...t, lens: "files" }));
      }
      return;
    }
    pendingRevealLineRef.current = line ?? null;
    setSelectedFile(target);
  }

  function notifyLineOutsideDiff(line: number) {
    addToast("info", `Line ${line} doesn't exist in this file's current version.`);
  }

  /** Switch `tabId` to `lens`. The single place lens transitions happen —
   * switcher clicks, keyboard 1/2/3, deep links, and chat actions all route
   * through this (or through setSelectedFile/handleViewCommit, which set
   * their matching lens directly since they already know the target).
   * Entering Files with nothing selected auto-picks the first unviewed file
   * in guided order (falling back to the first file); entering Commits with
   * no commit scoped auto-picks the most recent commit and kicks off its
   * diff fetch via handleViewCommit. */
  function setLens(tabId: string, lens: PrLens) {
    const tab = tabsRef.current.find((t) => t.id === tabId);
    if (!tab?.manifest) return;
    if (lens === "files" && !tab.selectedFile) {
      const order = guidedOrder(tab);
      const firstPath = nextUnviewed(order, -1, undefined, tab)?.path ?? order[0];
      const first = firstPath ? tab.manifest.files.find((f) => f.path === firstPath) : undefined;
      ctx.updateTab(tabId, (t) => ({ ...t, lens, selectedFile: first ?? t.selectedFile }));
      return;
    }
    if (lens === "commits" && !tab.selectedCommit) {
      const commits = tab.manifest.commits;
      const first = commits[commits.length - 1];
      if (first) { ctx.handleViewCommit(first, tabId); return; }
    }
    ctx.updateTab(tabId, (t) => ({ ...t, lens }));
  }

  // The single low-level "select a file" primitive — every open-file path
  // (sidebar clicks, search, chat citations, top-risk rows, next/prev,
  // guided review) routes through this, so always landing in the Files lens
  // needs no per-callsite changes (issue #170).
  function setSelectedFile(file: FileDiff) {
    ctx.updateTab(activeTabId, (t) => ({ ...t, selectedFile: file, lens: "files" }));
  }

  /** A change-group row on the Overview — scopes the Files sidebar to the
   * group and opens it at the group's first file (issue #170). */
  function openGroup(group: ChangeGroup, files: FileDiff[]) {
    if (!activeTabId || files.length === 0) return;
    ctx.updateTab(activeTabId, (t) => ({ ...t, groupFilter: group.label, lens: "files", selectedFile: files[0] }));
  }

  function handleViewChange(view: SidebarView) {
    ctx.updateTab(activeTabId,(t) => ({ ...t, sidebarView: view }));
  }

  return {
    guidedOrder,
    triageRationale,
    nextUnviewed,
    markReviewedAndAdvance,
    selectAdjacentFile,
    resolveManifestFile,
    handleChatOpenFile,
    notifyLineOutsideDiff,
    setLens,
    setSelectedFile,
    openGroup,
    handleViewChange,
  };
}

export type NavigationApi = ReturnType<typeof createNavigation>;
