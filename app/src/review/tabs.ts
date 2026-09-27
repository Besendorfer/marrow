// Tabs handlers (moved verbatim from App.tsx in issue #238 phase 2).
// Calls into other modules go through `ctx`, which holds every handler of
// the current render — the same per-render closure semantics as before.

import { exit } from "@tauri-apps/plugin-process";
import type { ReviewManifest, Tab } from "../types";
import { isOpenerTab, emptyChatState } from "./helpers";
import type { ReviewCtx } from "./ctx";

// `ctxArg` is typed unknown only so ReturnType<typeof create…> (which
// ReviewCtx is built from) doesn't loop through this parameter's type.
export function createTabs(ctxArg: unknown) {
  const ctx = ctxArg as ReviewCtx;
  const {
    nextTabId,
    tabs,
    setTabs,
    activeTabId,
    setActiveTabId,
    setChecksMap,
    setChatActionStatuses,
    chatExecutedActionsRef,
  } = ctx;

  function selectAdjacentTab(delta: 1 | -1) {
    if (tabs.length < 2) return;
    const i = tabs.findIndex((t) => t.id === activeTabId);
    if (i < 0) return;
    handleSelectTab(tabs[(i + delta + tabs.length) % tabs.length].id);
  }

  function buildReviewTab(id: string, manifest: ReviewManifest): Tab {
    const hasGroups = (manifest.change_groups ?? []).length > 0;
    return {
      id,
      manifest,
      loading: null,
      // Land on the overview (summary + change groups), not a file — the
      // "Start review" CTA and sidebar are the ways in.
      selectedFile: null,
      lens: "overview",
      selectedCommit: null,
      groupFilter: null,
      viewedFiles: new Set(),
      staleViewedFiles: new Set(),
      dismissedHighlights: new Set(),
      noteResolutions: new Map(),
      resolvedSpecKeys: new Set(),
      specResolutions: new Map(),
      localRequirements: null,
      analyzingRequirements: false,
      chat: emptyChatState(),
      commentThreads: { status: "idle" },
      checkAnnotations: { status: "idle" },
      commentsOpen: false,
      prConversation: null,
      prCommentDraft: null,
      sidebarView: hasGroups ? "groups" : "category",
      isRefreshing: false,
      lastCommentCount: 0,
    };
  }

  function createTab(manifest: ReviewManifest): Tab {
    return buildReviewTab(String(nextTabId.current++), manifest);
  }

  // A tab that hasn't loaded a PR yet — renders the opener form (or the loading
  // view once a fetch starts in it).
  function createOpenerTab(): Tab {
    return {
      id: String(nextTabId.current++),
      manifest: null,
      loading: null,
      error: null,
      selectedFile: null,
      lens: "overview",
      selectedCommit: null,
      groupFilter: null,
      viewedFiles: new Set(),
      staleViewedFiles: new Set(),
      dismissedHighlights: new Set(),
      noteResolutions: new Map(),
      resolvedSpecKeys: new Set(),
      specResolutions: new Map(),
      localRequirements: null,
      analyzingRequirements: false,
      chat: emptyChatState(),
      commentThreads: { status: "idle" },
      checkAnnotations: { status: "idle" },
      commentsOpen: false,
      prConversation: null,
      prCommentDraft: null,
      sidebarView: "category",
      isRefreshing: false,
      lastCommentCount: 0,
    };
  }

  function handleNewReview() {
    const tab = createOpenerTab();
    setTabs((prev) => [...prev, tab]);
    setActiveTabId(tab.id);
  }

  function handleSelectTab(id: string) {
    setActiveTabId(id);
    // Viewing a tab clears its "finished loading" notification.
    setTabs((prev) => prev.map((t) => (t.id === id && t.unread ? { ...t, unread: false } : t)));
  }

  function updateTab(tabId: string | null, updater: (tab: Tab) => Tab) {
    setTabs((prev) => prev.map((t) => (t.id === tabId ? updater(t) : t)));
  }

  function closeTab(tabId: string) {
    const idx = tabs.findIndex((t) => t.id === tabId);
    const closing = tabs.find((t) => t.id === tabId);
    let next = tabs.filter((t) => t.id !== tabId);
    setChecksMap((prev) => {
      const copy = { ...prev };
      delete copy[tabId];
      return copy;
    });
    setChatActionStatuses((prev) => {
      const copy = { ...prev };
      delete copy[tabId];
      return copy;
    });
    delete chatExecutedActionsRef.current[tabId];
    if (next.length === 0) {
      // Closing the last *review* tab drops back to a fresh opener tab so the tab
      // bar (and a way to open a PR) is always present. But closing the last tab
      // when it's already an empty opener means the user is done — quit the app.
      if (closing && isOpenerTab(closing)) {
        exit(0);
        return;
      }
      const opener = createOpenerTab();
      next = [opener];
      setTabs(next);
      setActiveTabId(opener.id);
      return;
    }
    setTabs(next);
    if (tabId === activeTabId) {
      const newIdx = Math.min(idx, next.length - 1);
      setActiveTabId(next[newIdx].id);
    }
  }

  return {
    selectAdjacentTab,
    buildReviewTab,
    createTab,
    createOpenerTab,
    handleNewReview,
    handleSelectTab,
    updateTab,
    closeTab,
  };
}

export type TabsApi = ReturnType<typeof createTabs>;
