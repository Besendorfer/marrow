// Loading handlers (moved verbatim from App.tsx in issue #238 phase 2).
// Calls into other modules go through `ctx`, which holds every handler of
// the current render — the same per-render closure semantics as before.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ReviewManifest, Tab, FetchProgress, PrUpdateStatus, CachedPrInfo } from "../types";
import { canonicalPrKey } from "../utils";
import { collectHighlightKeys } from "./helpers";
import type { ReviewCtx } from "./ctx";

// `ctxArg` is typed unknown only so ReturnType<typeof create…> (which
// ReviewCtx is built from) doesn't loop through this parameter's type.
export function createLoading(ctxArg: unknown) {
  const ctx = ctxArg as ReviewCtx;
  const {
    setTabs,
    activeTabId,
    setActiveTabId,
    setError,
    setStaleConfirm,
    fetchingRef,
    fetchTokenRef,
    addToast,
    activeTab,
    unlistenRef,
    tabsRef,
    activeTabIdRef,
  } = ctx;

  // Opening from the "Recently analyzed" cache: if the PR's head moved, the
  // backend would silently run a full AI re-analysis — surface that first.
  async function handleOpenCachedPr(prRef: string, info: CachedPrInfo) {
    try {
      const status = await invoke<PrUpdateStatus>("check_pr_updates", {
        prUrl: info.pr_url,
        currentHeadSha: info.head_sha,
        currentCommentCount: 0,
      });
      if (status.head_sha_changed) {
        setStaleConfirm({ prRef, title: info.pr_title });
        return;
      }
    } catch {
      // Status check failing (offline, rate limit) shouldn't block opening.
    }
    handleFetchStart(prRef);
  }

  async function loadManifest(path: string) {
    try {
      const data = await invoke<ReviewManifest>("load_manifest", { path });
      handleManifestLoaded(data);
    } catch (e) {
      setError(String(e));
    }
  }

  // Turn a pending (opener/loading) tab into a loaded review tab in place,
  // preserving its position in the tab bar. If the user has navigated away to
  // another tab, we don't steal focus — instead the tab is flagged unread and a
  // toast lets them know it finished.
  function fillTabWithManifest(tabId: string, data: ReviewManifest) {
    const isActive = activeTabIdRef.current === tabId;
    const tab = ctx.buildReviewTab(tabId, data);
    if (!isActive) tab.unread = true;
    setTabs((prev) => prev.map((t) => (t.id === tabId ? tab : t)));
    setError(null);
    ctx.loadPersistedViewedState(tab);
    ctx.loadDismissedHighlights(tab);
    ctx.loadResolvedSpecs(tab);
    ctx.loadLocalRequirements(tab);
    ctx.loadChatHistory(tab);
    ctx.fetchMyReviewState(tabId, data.pr_url);
    ctx.fetchChecksStatus(tabId, data.pr_url);
    if (!isActive) {
      addToast("success", `PR #${data.pr_number} finished loading`);
    }
  }

  function handleManifestLoaded(data: ReviewManifest) {
    // Reuse the active opener tab if one is focused; otherwise open a new tab.
    if (activeTab && activeTab.manifest === null && !activeTab.loading) {
      fillTabWithManifest(activeTab.id, data);
      return;
    }
    const tab = ctx.createTab(data);
    setTabs((prev) => [...prev, tab]);
    setActiveTabId(tab.id);
    setError(null);
    ctx.loadPersistedViewedState(tab);
    ctx.loadDismissedHighlights(tab);
    ctx.loadResolvedSpecs(tab);
    ctx.loadLocalRequirements(tab);
    ctx.loadChatHistory(tab);
    ctx.fetchMyReviewState(tab.id, data.pr_url);
    ctx.fetchChecksStatus(tab.id, data.pr_url);
  }

  // Fetch a PR into a tab. If `targetTabId` is an existing opener tab it loads
  // in place; otherwise a new tab is created so opening never takes over the
  // currently active review.
  async function handleFetchStart(prRef: string, targetTabId?: string) {
    // If this PR is already open in a tab, just switch to it rather than
    // fetching it again into a new tab.
    const key = canonicalPrKey(prRef);
    if (key) {
      const alreadyOpen = tabsRef.current.find(
        (t) => t.manifest && canonicalPrKey(t.manifest.pr_url) === key
      );
      if (alreadyOpen) {
        // handleSelectTab (not bare setActiveTabId) also clears the tab's
        // unread badge, matching the deep-link open path.
        ctx.handleSelectTab(alreadyOpen.id);
        return;
      }
    }

    if (fetchingRef.current) return;
    fetchingRef.current = true;
    const token = ++fetchTokenRef.current;
    setError(null);

    let tabId = targetTabId;
    const existing = tabId ? tabsRef.current.find((t) => t.id === tabId) : undefined;
    if (!existing || existing.manifest !== null) {
      // No reusable opener tab — spin up a fresh one and switch to it.
      const tab = ctx.createOpenerTab();
      setTabs((prev) => [...prev, tab]);
      setActiveTabId(tab.id);
      tabId = tab.id;
    }
    const loadingTabId = tabId!;

    ctx.updateTab(loadingTabId, (t) => ({
      ...t,
      error: null,
      lastPrRef: prRef,
      loading: { prRef, prTitle: null, progress: null, fileCounts: {} },
    }));

    const unlisten = await listen<FetchProgress>("fetch-progress", (event) => {
      ctx.updateTab(loadingTabId, (t) => {
        if (!t.loading) return t;
        const fileCounts =
          event.payload.files_total != null
            ? {
                ...t.loading.fileCounts,
                [event.payload.step]: {
                  done: event.payload.files_done ?? 0,
                  total: event.payload.files_total,
                },
              }
            : t.loading.fileCounts;
        return {
          ...t,
          loading: {
            ...t.loading,
            progress: event.payload,
            prTitle: event.payload.pr_title ?? t.loading.prTitle,
            fileCounts,
          },
        };
      });
    });
    unlistenRef.current = unlisten;

    try {
      const manifest = await invoke<ReviewManifest>("fetch_pr", { prRef });
      // Cancelled or superseded while in flight — drop the result.
      if (fetchTokenRef.current !== token) return;
      fillTabWithManifest(loadingTabId, manifest);
    } catch (err) {
      if (fetchTokenRef.current !== token) return;
      const message = String(err);
      const isActive = activeTabIdRef.current === loadingTabId;
      // Keep the failure in its own tab rather than hijacking the whole window.
      // If the user has moved on, flag the tab and toast instead of stealing focus.
      ctx.updateTab(loadingTabId, (t) => ({
        ...t,
        loading: null,
        error: message,
        unread: isActive ? t.unread : true,
      }));
      if (!isActive) {
        addToast("error", `PR failed to load: ${message}`);
      }
    } finally {
      unlisten();
      if (unlistenRef.current === unlisten) unlistenRef.current = null;
      if (fetchTokenRef.current === token) fetchingRef.current = false;
    }
  }

  function handleFetchCancel(tabId: string) {
    unlistenRef.current?.();
    unlistenRef.current = null;
    // Invalidate the in-flight fetch so its late result is ignored, and free up
    // the fetch guard so the user can open another PR immediately.
    fetchTokenRef.current++;
    fetchingRef.current = false;
    ctx.updateTab(tabId, (t) => ({ ...t, loading: null }));
  }

  async function handleRefreshPr(tabId?: string) {
    const targetId = tabId ?? activeTabId;
    const tab = tabsRef.current.find((t) => t.id === targetId);
    if (!tab || !tab.manifest || tab.isRefreshing || fetchingRef.current) return;

    ctx.updateTab(tab.id, (t) => ({ ...t, isRefreshing: true }));

    try {
      const newManifest = await invoke<ReviewManifest>("fetch_pr", {
        prRef: tab.manifest.pr_url,
      });

      const parts: string[] = [];
      if (newManifest.head_sha !== tab.manifest.head_sha) {
        parts.push("new commits");
      }
      const oldPaths = new Set(tab.manifest.files.map((f) => f.path));
      const newPaths = new Set(newManifest.files.map((f) => f.path));
      const added = newManifest.files.filter((f) => !oldPaths.has(f.path));
      const removed = tab.manifest.files.filter((f) => !newPaths.has(f.path));
      if (added.length > 0) parts.push(`${added.length} file${added.length > 1 ? "s" : ""} added`);
      if (removed.length > 0) parts.push(`${removed.length} file${removed.length > 1 ? "s" : ""} removed`);

      const newFileHashMap = new Map(newManifest.files.map((f) => [f.path, f.diff_hash]));
      const oldFileHashMap = new Map(tab.manifest.files.map((f) => [f.path, f.diff_hash]));

      // Build saved-hash map from currently viewed files for reconciliation
      const savedFiles: Record<string, string> = {};
      for (const viewedPath of tab.viewedFiles) {
        const hash = oldFileHashMap.get(viewedPath);
        if (hash) savedFiles[viewedPath] = hash;
      }
      const { viewed: preservedViewed, stale: reconciledStale } = ctx.reconcileViewedFiles(savedFiles, newFileHashMap);

      // Carry forward existing stale entries (minus files removed from PR)
      const newStale = new Set<string>(tab.staleViewedFiles);
      for (const path of reconciledStale) newStale.add(path);
      for (const stalePath of newStale) {
        if (!newFileHashMap.has(stalePath)) newStale.delete(stalePath);
      }

      const staleCount = newStale.size - tab.staleViewedFiles.size;
      if (staleCount > 0) parts.push(`${staleCount} file${staleCount > 1 ? "s" : ""} changed since reviewed`);

      // Re-analysis may surface highlights the previous pass didn't — diff the
      // key sets so the overview/diff can call out what's new since last refresh.
      const oldHighlightKeys = collectHighlightKeys(tab.manifest);
      const newHighlightKeys = new Set(
        [...collectHighlightKeys(newManifest)].filter((k) => !oldHighlightKeys.has(k))
      );

      const refreshedTab: Tab = {
        ...tab,
        manifest: newManifest,
        isRefreshing: false,
        viewedFiles: preservedViewed,
        staleViewedFiles: newStale,
        // No selection (the overview) stays on the overview after a refresh;
        // a selected file follows to the refreshed manifest if it still exists.
        selectedFile:
          tab.selectedFile == null
            ? null
            : newPaths.has(tab.selectedFile.path)
              ? newManifest.files.find((f) => f.path === tab.selectedFile!.path) ?? null
              : newManifest.files[0] ?? null,
        commentThreads: { status: "idle" },
        // A refresh can land on a new head_sha, invalidating any annotations
        // fetched for the old one — re-fetch is driven by the idle-state effect.
        checkAnnotations: { status: "idle" },
        newHighlightKeys,
        // A change-group filter naming a group the re-analysis dropped would
        // otherwise linger invisibly and silently reapply if a same-labeled
        // group ever came back — clear it rather than carry a dangling scope.
        groupFilter:
          tab.groupFilter && (newManifest.change_groups ?? []).some((g) => g.label === tab.groupFilter)
            ? tab.groupFilter
            : null,
      };

      ctx.updateTab(tab.id, () => refreshedTab);
      ctx.persistViewedState(refreshedTab);
      ctx.fetchMyReviewState(tab.id, newManifest.pr_url);
      ctx.fetchChecksStatus(tab.id, newManifest.pr_url);
      ctx.syncGhViewedState(tab.id, newManifest.pr_url, newManifest.files);

      if (parts.length > 0) {
        addToast("success", `PR updated: ${parts.join(", ")}`);
      } else {
        addToast("info", "PR refreshed — no changes detected");
      }
      if (newHighlightKeys.size > 0) {
        addToast("info", `${newHighlightKeys.size} new AI ${newHighlightKeys.size === 1 ? "note" : "notes"} from re-analysis`);
      }
    } catch (err) {
      ctx.updateTab(tab.id, (t) => ({ ...t, isRefreshing: false }));
      addToast("error", `Refresh failed: ${String(err)}`);
    }
  }

  async function handleFileDrop(e: React.DragEvent) {
    e.preventDefault();
    const file = e.dataTransfer.files[0];
    if (file) {
      const path = (file as File & { path?: string }).path;
      if (path) {
        loadManifest(path);
      }
    }
  }

  return {
    handleOpenCachedPr,
    loadManifest,
    fillTabWithManifest,
    handleManifestLoaded,
    handleFetchStart,
    handleFetchCancel,
    handleRefreshPr,
    handleFileDrop,
  };
}

export type LoadingApi = ReturnType<typeof createLoading>;
