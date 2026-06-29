import { useState, useEffect, useRef, useMemo, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { FileSidebar } from "./components/FileSidebar";
import { DiffViewer, detectLanguage, type DiffViewerHandle } from "./components/DiffViewer";
import { CommentsViewer } from "./components/CommentsViewer";
import { Header } from "./components/Header";
import { PrOpener } from "./components/PrOpener";
import { ReviewRequestList } from "./components/ReviewRequestList";
import { ActivityWidget } from "./components/ActivityWidget";
import { LoadingView } from "./components/LoadingView";
import { SettingsModal } from "./components/SettingsModal";
import { ChecksBlockingModal } from "./components/ChecksBlockingModal";
import { SummaryParagraphs } from "./components/SummaryParagraphs";
import { SearchBar, type SearchBarHandle } from "./components/SearchBar";
import { KeyboardHelp } from "./components/KeyboardHelp";
import { ReviewPicker } from "./components/ReviewPicker";
import { ToastContainer, createToast, type ToastData } from "./components/Toast";
import { UpdateBanner } from "./components/UpdateBanner";
import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch, exit } from "@tauri-apps/plugin-process";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { ReviewManifest, FileDiff, DiffViewMode, Tab, FetchProgress, HunkSignificanceFilter, SidebarView, ReviewThread, ReviewComment, SearchMatch, PrUpdateStatus, ViewedFileState, MyReviewState, PrChecksStatus, UpdateStatus, SessionState, Settings } from "./types";
import { parsePrUrl, extractPrRef, canonicalPrKey } from "./utils";

/** An empty "open a PR" tab — no loaded PR, not mid-fetch, no error. */
function isOpenerTab(tab: Tab): boolean {
  return !tab.manifest && !tab.loading && !tab.error;
}

function App() {
  const nextTabId = useRef(1);
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeTabId, setActiveTabId] = useState<string | null>(null);
  const [viewMode, setViewMode] = useState<DiffViewMode>("split");
  const [showHunkSignificance, setShowHunkSignificance] = useState(true);
  const [showAiNotes, setShowAiNotes] = useState(true);
  const [hunkFilter, setHunkFilter] = useState<HunkSignificanceFilter>("all");
  const [error, setError] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const [reviewPickerOpen, setReviewPickerOpen] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const searchRef = useRef<SearchBarHandle>(null);
  const diffViewerRef = useRef<DiffViewerHandle>(null);
  // Visible file order from the sidebar, used by the [ / ] navigation shortcuts.
  const visibleOrderRef = useRef<string[]>([]);
  const handleVisibleFilesChange = useCallback((paths: string[]) => {
    visibleOrderRef.current = paths;
  }, []);
  const [searchMatches, setSearchMatches] = useState<SearchMatch[]>([]);
  const [searchCurrentIndex, setSearchCurrentIndex] = useState(0);
  const [searchQuery, setSearchQuery] = useState("");
  const [toasts, setToasts] = useState<ToastData[]>([]);
  const [checksMap, setChecksMap] = useState<Record<string, PrChecksStatus>>({});
  const [checksDismissed, setChecksDismissed] = useState<Record<string, boolean>>({});
  const [updateStatus, setUpdateStatus] = useState<UpdateStatus>({ state: "idle" });
  const updateStatusRef = useRef(updateStatus.state);
  updateStatusRef.current = updateStatus.state;
  const pendingUpdateRef = useRef<Awaited<ReturnType<typeof check>>>(null);
  const sessionRestoredRef = useRef(false);
  const settingsRef = useRef<Settings | null>(null);
  // True while a PR fetch is in flight (guards refresh/polling/concurrent fetches).
  const fetchingRef = useRef(false);
  // Monotonic token identifying the active fetch; bumped on cancel so a
  // superseded/cancelled fetch resolving late is dropped instead of filling a tab.
  const fetchTokenRef = useRef(0);

  const handleSettingsClose = useCallback(() => {
    setSettingsOpen(false);
    invoke<Settings>("get_settings").then((s) => { settingsRef.current = s; }).catch(() => {});
  }, []);

  const addToast = useCallback((type: ToastData["type"], message: string) => {
    setToasts((prev) => [...prev, createToast(type, message)]);
  }, []);

  const removeToast = useCallback((id: string) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);


  const checkForUpdates = useCallback(async (silent = false) => {
    if (updateStatusRef.current === "checking" || updateStatusRef.current === "downloading") return;
    setUpdateStatus({ state: "checking" });
    try {
      const update = await check();
      if (update) {
        pendingUpdateRef.current = update;
        setUpdateStatus({ state: "available", version: update.version });
      } else {
        setUpdateStatus({ state: "up-to-date" });
        if (!silent) addToast("info", "You're on the latest version");
        setTimeout(() => setUpdateStatus((s) => s.state === "up-to-date" ? { state: "idle" } : s), 3000);
      }
    } catch (err) {
      setUpdateStatus({ state: "idle" });
      if (silent) return;
      const msg = String(err);
      if (msg.includes("Could not fetch") || msg.includes("404")) {
        addToast("info", "No releases published yet — updates will work once a release is available");
      } else {
        addToast("error", `Update check failed: ${msg}`);
      }
    }
  }, [addToast]);

  const handleDownloadUpdate = useCallback(async () => {
    const update = pendingUpdateRef.current;
    if (!update || updateStatusRef.current !== "available") return;
    setUpdateStatus({ state: "downloading", progress: 0 });
    try {
      let totalBytes = 0;
      let downloadedBytes = 0;
      await update.downloadAndInstall((event) => {
        if (event.event === "Started" && event.data.contentLength) {
          totalBytes = event.data.contentLength;
        } else if (event.event === "Progress") {
          downloadedBytes += event.data.chunkLength;
          const pct = totalBytes > 0 ? Math.round((downloadedBytes / totalBytes) * 100) : 0;
          setUpdateStatus({ state: "downloading", progress: pct });
        } else if (event.event === "Finished") {
          setUpdateStatus({ state: "ready" });
        }
      });
      setUpdateStatus({ state: "ready" });
    } catch (err) {
      setUpdateStatus({ state: "idle" });
      addToast("error", `Update download failed: ${String(err)}`);
    }
  }, [addToast]);

  useEffect(() => {
    if (import.meta.env.DEV) return;
    const startupTimer = setTimeout(() => checkForUpdates(true), 5000);
    const interval = setInterval(() => checkForUpdates(true), 6 * 60 * 60 * 1000);
    return () => {
      clearTimeout(startupTimer);
      clearInterval(interval);
    };
  }, [checkForUpdates]);

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? null;
  const openPrUrls = useMemo(
    () => new Set(tabs.filter((t) => t.manifest).map((t) => t.manifest!.pr_url)),
    [tabs]
  );

  const activeChecks = activeTab ? checksMap[activeTab.id] : undefined;
  const showChecksModal = !!activeTab && !!activeTab.manifest && !!activeChecks && activeChecks.overall_state !== "success" && !checksDismissed[activeTab.manifest.pr_url];

  const selectedFilePath = activeTab?.selectedFile?.path ?? null;
  const fileSearchMatches = useMemo(
    () => searchMatches.filter((m) => m.filePath === selectedFilePath),
    [searchMatches, selectedFilePath]
  );
  const currentSearchMatch = useMemo(
    () => {
      const m = searchMatches[searchCurrentIndex];
      return m?.filePath === selectedFilePath ? m : null;
    },
    [searchMatches, searchCurrentIndex, selectedFilePath]
  );

  // ── Keyboard shortcuts (ported from the CLI/TUI; see useKeyboardShortcuts) ──
  function selectAdjacentFile(delta: 1 | -1) {
    if (!activeTab?.manifest || !activeTab.selectedFile) return;
    const order = visibleOrderRef.current.length
      ? visibleOrderRef.current
      : activeTab.manifest.files.map((f) => f.path);
    const byPath = (p: string) => activeTab.manifest!.files.find((f) => f.path === p);
    const i = order.indexOf(activeTab.selectedFile.path);
    if (i === -1) {
      // Current file is filtered out of the list — jump to the first visible one.
      const first = byPath(order[0]);
      if (first) setSelectedFile(first);
      return;
    }
    const next = byPath(order[Math.min(Math.max(i + delta, 0), order.length - 1)]);
    if (next && next.path !== activeTab.selectedFile.path) setSelectedFile(next);
  }

  function selectAdjacentTab(delta: 1 | -1) {
    if (tabs.length < 2) return;
    const i = tabs.findIndex((t) => t.id === activeTabId);
    if (i < 0) return;
    handleSelectTab(tabs[(i + delta + tabs.length) % tabs.length].id);
  }

  function toggleThreadsView() {
    if (!activeTab?.manifest) return;
    if (activeTab.sidebarView === "comments") {
      const hasGroups = (activeTab.manifest.change_groups ?? []).length > 0;
      handleViewChange(hasGroups ? "groups" : "category");
    } else {
      handleViewChange("comments");
    }
  }

  useKeyboardShortcuts(
    {
      onNextFile: () => selectAdjacentFile(1),
      onPrevFile: () => selectAdjacentFile(-1),
      onToggleViewed: () => {
        const path = activeTab?.selectedFile?.path;
        if (path) toggleViewed(path);
      },
      onToggleThreads: toggleThreadsView,
      onRefresh: () => { if (activeTab?.manifest) handleRefreshPr(); },
      onOpenSearch: () => searchRef.current?.open("local"),
      onToggleHelp: () => setHelpOpen((o) => !o),
      onCloseOverlays: () => { setHelpOpen(false); setReviewPickerOpen(false); },
      onNextTab: () => selectAdjacentTab(1),
      onPrevTab: () => selectAdjacentTab(-1),
      onCloseTab: () => { if (activeTabId) closeTab(activeTabId); },
      onNewTab: () => handleNewReview(),
      onNextHunk: () => diffViewerRef.current?.nextHunk(),
      onPrevHunk: () => diffViewerRef.current?.prevHunk(),
      onNextFinding: () => diffViewerRef.current?.nextFinding(),
      onPrevFinding: () => diffViewerRef.current?.prevFinding(),
      onFoldAll: () => diffViewerRef.current?.foldAll(),
      // Tier 3 — line cursor + actions
      onCursorDown: () => diffViewerRef.current?.cursorMove(1),
      onCursorUp: () => diffViewerRef.current?.cursorMove(-1),
      onCursorTop: () => diffViewerRef.current?.cursorEdge("top"),
      onCursorBottom: () => diffViewerRef.current?.cursorEdge("bottom"),
      onCursorHalfDown: () => diffViewerRef.current?.cursorPage(1, 0.5),
      onCursorHalfUp: () => diffViewerRef.current?.cursorPage(-1, 0.5),
      onCursorPageDown: () => diffViewerRef.current?.cursorPage(1, 0.9),
      onCursorPageUp: () => diffViewerRef.current?.cursorPage(-1, 0.9),
      onFoldAtCursor: () => diffViewerRef.current?.foldAtCursor(),
      onComment: () => diffViewerRef.current?.commentAtCursor(),
      onToggleAnchor: () => diffViewerRef.current?.toggleAnchor(),
      onReply: () => diffViewerRef.current?.replyAtCursor(),
      onResolve: () => diffViewerRef.current?.resolveAtCursor(),
      onReviewPicker: () => { if (activeTab?.manifest) setReviewPickerOpen(true); },
    },
    {
      enabled: !!activeTab?.manifest,
      overlayOpen: helpOpen || settingsOpen || searchOpen || showChecksModal || reviewPickerOpen,
    },
  );

  function buildReviewTab(id: string, manifest: ReviewManifest): Tab {
    const hasGroups = (manifest.change_groups ?? []).length > 0;
    return {
      id,
      manifest,
      loading: null,
      selectedFile: manifest.files.length > 0 ? manifest.files[0] : null,
      viewedFiles: new Set(),
      staleViewedFiles: new Set(),
      dismissedHighlights: new Set(),
      commentThreads: { status: "idle" },
      selectedCommentFile: null,
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
      viewedFiles: new Set(),
      staleViewedFiles: new Set(),
      dismissedHighlights: new Set(),
      commentThreads: { status: "idle" },
      selectedCommentFile: null,
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

  useEffect(() => {
    async function initSession() {
      // Load user preferences from settings
      try {
        const settings = await invoke<Settings>("get_settings");
        settingsRef.current = settings;
        setViewMode(settings.view_mode || "split");
        setShowHunkSignificance(settings.show_hunk_significance ?? true);
        setShowAiNotes(settings.show_ai_notes ?? true);
        setHunkFilter(settings.hunk_filter || "all");
      } catch {
        // Use defaults on failure
      }

      // Check for CLI manifest path first
      const cliPath = await invoke<string | null>("get_initial_manifest_path");
      if (cliPath) {
        sessionRestoredRef.current = true;
        loadManifest(cliPath);
        return;
      }

      // Check for deep link (cold-start: app launched via URL)
      const deepLink = await invoke<string | null>("get_pending_deep_link");

      // Restore previous session before honoring a deep link, so the user
      // doesn't lose their open PRs just because they clicked an external link.
      let restoredTabs: Tab[] = [];
      try {
        const session = await invoke<SessionState | null>("load_session");
        if (session && session.open_prs.length > 0) {
          const loaded = await Promise.all(
            session.open_prs.map(async (entry) => {
              try {
                const manifest = await invoke<ReviewManifest | null>(
                  "load_cached_manifest_by_pr",
                  { prUrl: entry.pr_url },
                );
                if (!manifest) return null;
                const tab = createTab(manifest);
                if (entry.selected_file) {
                  const file = manifest.files.find((f) => f.path === entry.selected_file);
                  if (file) tab.selectedFile = file;
                }
                if (entry.sidebar_view) {
                  tab.sidebarView = entry.sidebar_view as SidebarView;
                }
                if (entry.selected_comment_file) {
                  tab.selectedCommentFile = entry.selected_comment_file;
                }
                return tab;
              } catch {
                return null;
              }
            }),
          );
          const restored = loaded.filter((t): t is Tab => t !== null);

          if (restored.length > 0) {
            restoredTabs = restored;
            setTabs(restored);
            const active = session.active_pr
              ? restored.find((t) => t.manifest!.pr_url === session.active_pr)
              : null;
            setActiveTabId(active?.id ?? restored[0].id);

            for (const tab of restored) {
              loadPersistedViewedState(tab);
              loadDismissedHighlights(tab);
              fetchMyReviewState(tab.id, tab.manifest!.pr_url);
              fetchChecksStatus(tab.id, tab.manifest!.pr_url);
            }
          }
        }
      } catch {
        // Session restore is best-effort
      }

      sessionRestoredRef.current = true;

      if (deepLink) {
        // Open the incoming PR in its own new tab.
        handleFetchStart(deepLink);
      } else if (restoredTabs.length === 0) {
        // Nothing to show — start with an opener tab so the tab bar is present
        // from the start instead of a full-screen empty state.
        handleNewReview();
      }

      // Tell the backend it's safe to skip cold-start buffering — from now on
      // hot-open emits go straight to the listener above.
      invoke("signal_frontend_ready").catch(() => {});
    }

    initSession();
  }, []);

  // Listen for deep links while app is running (hot-open)
  useEffect(() => {
    const unlisten = listen<string>("deep-link-open", (event) => {
      // Receiving an emit proves the frontend is wired up — clear any
      // race-buffered duplicate (deep link fired between listener mount and
      // signal_frontend_ready) and re-assert ready so further hot-opens skip
      // buffering entirely.
      invoke("get_pending_deep_link").catch(() => {});
      invoke("signal_frontend_ready").catch(() => {});
      if (!event.payload) return;
      if (fetchingRef.current) {
        addToast("info", "Already fetching a PR — try the deep link again when it finishes.");
        return;
      }
      // Match by canonical owner/repo/pull/N rather than raw URL — payload format
      // (with/without scheme, www., trailing slash) may not match manifest pr_url verbatim.
      const incomingRef = extractPrRef(event.payload);
      if (incomingRef) {
        const existing = tabsRef.current.find(
          (t) => t.manifest && extractPrRef(t.manifest.pr_url) === incomingRef
        );
        if (existing) {
          handleSelectTab(existing.id);
          return;
        }
      }
      handleFetchStart(event.payload);
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

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
    const tab = buildReviewTab(tabId, data);
    if (!isActive) tab.unread = true;
    setTabs((prev) => prev.map((t) => (t.id === tabId ? tab : t)));
    setError(null);
    loadPersistedViewedState(tab);
    loadDismissedHighlights(tab);
    fetchMyReviewState(tabId, data.pr_url);
    fetchChecksStatus(tabId, data.pr_url);
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
    const tab = createTab(data);
    setTabs((prev) => [...prev, tab]);
    setActiveTabId(tab.id);
    setError(null);
    loadPersistedViewedState(tab);
    loadDismissedHighlights(tab);
    fetchMyReviewState(tab.id, data.pr_url);
    fetchChecksStatus(tab.id, data.pr_url);
  }

  async function fetchMyReviewState(tabId: string, prUrl: string) {
    try {
      const state = await invoke<MyReviewState>("get_my_review_state", { prUrl });
      updateTab(tabId, (t) => ({ ...t, myReviewState: state }));
    } catch {
      // Non-critical: if fetching fails, button stays enabled
    }
  }

  async function fetchChecksStatus(tabId: string, prUrl: string) {
    try {
      const [checks, dismissed] = await Promise.all([
        invoke<PrChecksStatus>("get_pr_checks", { prUrl }),
        invoke<boolean>("is_checks_dismissed", { prUrl }),
      ]);
      setChecksMap((prev) => ({ ...prev, [tabId]: checks }));
      if (dismissed) {
        setChecksDismissed((prev) => ({ ...prev, [prUrl]: true }));
      }
    } catch {
      // Non-critical: if fetching fails, don't block the review
    }
  }

  async function handleDismissChecks(prUrl: string) {
    setChecksDismissed((prev) => ({ ...prev, [prUrl]: true }));
    try {
      await invoke("dismiss_checks_warning", { prUrl });
    } catch {
      // Persistence failure is non-critical
    }
  }

  function reconcileViewedFiles(
    savedFiles: Record<string, string>,
    currentHashMap: Map<string, string>,
  ): { viewed: Set<string>; stale: Set<string> } {
    const viewed = new Set<string>();
    const stale = new Set<string>();
    for (const [path, savedHash] of Object.entries(savedFiles)) {
      const currentHash = currentHashMap.get(path);
      if (currentHash === undefined) continue;
      if (currentHash === savedHash) viewed.add(path); else stale.add(path);
    }
    return { viewed, stale };
  }

  async function loadPersistedViewedState(tab: Tab) {
    if (!tab.manifest) return;
    try {
      const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
      const saved = await invoke<ViewedFileState | null>("load_viewed_files", { owner, repo, prNumber: number });
      if (!saved) return;

      const currentHashMap = new Map(tab.manifest.files.map((f) => [f.path, f.diff_hash]));
      const { viewed: viewedFiles, stale: staleViewedFiles } = reconcileViewedFiles(saved.files, currentHashMap);

      updateTab(tab.id, (t) => ({ ...t, viewedFiles, staleViewedFiles }));
    } catch {
      // Graceful degradation: if loading fails, start with empty state
    }
    syncGhViewedState(tab.id, tab.manifest.pr_url, tab.manifest.files);
  }

  async function loadDismissedHighlights(tab: Tab) {
    if (!tab.manifest) return;
    try {
      const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
      const saved = await invoke<{ keys: string[] } | null>("load_dismissed_highlights", { owner, repo, prNumber: number });
      if (saved && saved.keys.length > 0) {
        updateTab(tab.id, (t) => ({ ...t, dismissedHighlights: new Set(saved.keys) }));
      }
    } catch {
      // Non-critical: start with nothing dismissed on failure
    }
  }

  function toggleHighlightDismissed(key: string) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    const next = new Set(tab.dismissedHighlights);
    if (next.has(key)) next.delete(key); else next.add(key);
    updateTab(tab.id, (t) => ({ ...t, dismissedHighlights: next }));
    const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
    invoke("save_dismissed_highlights", { owner, repo, prNumber: number, state: { keys: [...next] } })
      .catch(() => addToast("error", "Couldn't save — this dismissal may not persist"));
  }

  const unlistenRef = useRef<(() => void) | null>(null);

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
        setActiveTabId(alreadyOpen.id);
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
      const tab = createOpenerTab();
      setTabs((prev) => [...prev, tab]);
      setActiveTabId(tab.id);
      tabId = tab.id;
    }
    const loadingTabId = tabId!;

    updateTab(loadingTabId, (t) => ({
      ...t,
      error: null,
      loading: { prRef, prTitle: null, progress: null, fileCounts: {} },
    }));

    const unlisten = await listen<FetchProgress>("fetch-progress", (event) => {
      updateTab(loadingTabId, (t) => {
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
      updateTab(loadingTabId, (t) => ({
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
    updateTab(tabId, (t) => ({ ...t, loading: null }));
  }

  const tabsRef = useRef(tabs);
  tabsRef.current = tabs;
  const activeTabIdRef = useRef(activeTabId);
  activeTabIdRef.current = activeTabId;
  // Latest tab handlers in refs so the once-mounted menu listeners never act on stale state.
  const closeTabRef = useRef<(id: string) => void>(() => {});
  const newTabRef = useRef<() => void>(() => {});
  newTabRef.current = handleNewReview;
  // Chrome-style confirm-quit: first Cmd+Q arms a hint, a second within the window quits.
  const [showQuitHint, setShowQuitHint] = useState(false);
  const quitArmedRef = useRef(false);
  const quitTimerRef = useRef<number | null>(null);
  const checksMapRef = useRef(checksMap);
  checksMapRef.current = checksMap;
  const checksDismissedRef = useRef(checksDismissed);
  checksDismissedRef.current = checksDismissed;

  function updateTab(tabId: string | null, updater: (tab: Tab) => Tab) {
    setTabs((prev) => prev.map((t) => (t.id === tabId ? updater(t) : t)));
  }

  async function handleRefreshPr(tabId?: string) {
    const targetId = tabId ?? activeTabId;
    const tab = tabsRef.current.find((t) => t.id === targetId);
    if (!tab || !tab.manifest || tab.isRefreshing || fetchingRef.current) return;

    updateTab(tab.id, (t) => ({ ...t, isRefreshing: true }));

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
      const { viewed: preservedViewed, stale: reconciledStale } = reconcileViewedFiles(savedFiles, newFileHashMap);

      // Carry forward existing stale entries (minus files removed from PR)
      const newStale = new Set<string>(tab.staleViewedFiles);
      for (const path of reconciledStale) newStale.add(path);
      for (const stalePath of newStale) {
        if (!newFileHashMap.has(stalePath)) newStale.delete(stalePath);
      }

      const staleCount = newStale.size - tab.staleViewedFiles.size;
      if (staleCount > 0) parts.push(`${staleCount} file${staleCount > 1 ? "s" : ""} changed since reviewed`);

      const refreshedTab: Tab = {
        ...tab,
        manifest: newManifest,
        isRefreshing: false,
        viewedFiles: preservedViewed,
        staleViewedFiles: newStale,
        selectedFile:
          tab.selectedFile && newPaths.has(tab.selectedFile.path)
            ? newManifest.files.find((f) => f.path === tab.selectedFile!.path) ?? newManifest.files[0] ?? null
            : newManifest.files[0] ?? null,
        commentThreads: { status: "idle" },
      };

      updateTab(tab.id, () => refreshedTab);
      persistViewedState(refreshedTab);
      fetchMyReviewState(tab.id, newManifest.pr_url);
      fetchChecksStatus(tab.id, newManifest.pr_url);
      syncGhViewedState(tab.id, newManifest.pr_url, newManifest.files);

      if (parts.length > 0) {
        addToast("success", `PR updated: ${parts.join(", ")}`);
      } else {
        addToast("info", "PR refreshed — no changes detected");
      }
    } catch (err) {
      updateTab(tab.id, (t) => ({ ...t, isRefreshing: false }));
      addToast("error", `Refresh failed: ${String(err)}`);
    }
  }

  async function handleRefreshComments(tabId: string) {
    const tab = tabsRef.current.find((t) => t.id === tabId);
    if (!tab || !tab.manifest || tab.isRefreshing) return;

    try {
      const threads = await invoke<ReviewThread[]>("fetch_review_comments", {
        prUrl: tab.manifest.pr_url,
      });

      const oldCount =
        tab.commentThreads.status === "loaded"
          ? tab.commentThreads.threads.reduce((n, t) => n + t.comments.length, 0)
          : 0;
      const newCount = threads.reduce((n, t) => n + t.comments.length, 0);
      const diff = newCount - oldCount;

      updateTab(tabId, (t) => ({
        ...t,
        commentThreads: { status: "loaded", threads },
        lastCommentCount: newCount,
      }));

      if (diff > 0) {
        addToast("info", `${diff} new comment${diff > 1 ? "s" : ""} on PR #${tab.manifest.pr_number}`);
      }
    } catch {
      // Poll will retry
    }
  }

  function setSelectedFile(file: FileDiff) {
    updateTab(activeTabId,(t) => ({ ...t, selectedFile: file }));
  }

  function toggleViewed(filePath: string) {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    const nowViewed = !tab.viewedFiles.has(filePath);
    const nextViewed = new Set(tab.viewedFiles);
    const nextStale = new Set(tab.staleViewedFiles);
    if (nowViewed) {
      nextViewed.add(filePath);
      nextStale.delete(filePath);
    } else {
      nextViewed.delete(filePath);
    }
    const updated = { ...tab, viewedFiles: nextViewed, staleViewedFiles: nextStale };
    updateTab(tab.id, () => updated);
    persistViewedState(updated);
    invoke("sync_file_viewed_to_github", { prUrl: tab.manifest.pr_url, path: filePath, viewed: nowViewed }).catch(() => {});
  }

  async function persistViewedState(tab: Tab) {
    if (!tab.manifest) return;
    try {
      const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
      const fileHashMap = new Map(tab.manifest.files.map((f) => [f.path, f.diff_hash]));
      const files: Record<string, string> = {};
      for (const path of tab.viewedFiles) {
        const hash = fileHashMap.get(path);
        if (hash) files[path] = hash;
      }
      await invoke("save_viewed_files", { owner, repo, prNumber: number, state: { files } });
    } catch {
      // Non-critical: persistence failure shouldn't block UI
    }
  }

  async function syncGhViewedState(tabId: string, prUrl: string, files: Array<{ path: string; diff_hash: string }>) {
    try {
      const ghState = await invoke<Record<string, string>>("fetch_gh_viewed_state", { prUrl });
      const currentPaths = new Set(files.map((f) => f.path));

      setTabs((prev) => {
        const tab = prev.find((t) => t.id === tabId);
        if (!tab) return prev;

        const nextViewed = new Set(tab.viewedFiles);
        const nextStale = new Set(tab.staleViewedFiles);
        let changed = false;

        for (const [path, state] of Object.entries(ghState)) {
          if (!currentPaths.has(path)) continue;
          if (state === "VIEWED" && !nextViewed.has(path) && !nextStale.has(path)) {
            nextViewed.add(path);
            changed = true;
          } else if (state === "UNVIEWED" && nextViewed.has(path)) {
            nextViewed.delete(path);
            changed = true;
          } else if (state === "DISMISSED" && nextViewed.has(path)) {
            nextViewed.delete(path);
            nextStale.add(path);
            changed = true;
          }
        }

        if (!changed) return prev;
        const updated = { ...tab, viewedFiles: nextViewed, staleViewedFiles: nextStale };
        persistViewedState(updated);
        return prev.map((t) => (t.id === tabId ? updated : t));
      });
    } catch {
      // GH sync is best-effort — skip on failure (no token, network error, etc.)
    }
  }

  function handleViewChange(view: SidebarView) {
    updateTab(activeTabId,(t) => ({ ...t, sidebarView: view }));
    if (view === "comments") {
      handleRequestComments();
    }
  }

  async function handleRequestComments() {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest || tab.commentThreads.status === "loading" || tab.commentThreads.status === "loaded") return;

    updateTab(activeTabId,(t) => ({ ...t, commentThreads: { status: "loading" } }));
    try {
      const threads = await invoke<ReviewThread[]>("fetch_review_comments", {
        prUrl: tab.manifest.pr_url,
      });
      // Set first file with comments as selected if none selected
      const firstFile = threads.length > 0 ? threads[0].path : null;
      setTabs((prev) =>
        prev.map((t) =>
          t.id === activeTabId
            ? {
                ...t,
                commentThreads: { status: "loaded", threads },
                selectedCommentFile: t.selectedCommentFile ?? firstFile,
              }
            : t
        )
      );
    } catch (err) {
      updateTab(activeTabId,(t) => ({
        ...t,
        commentThreads: { status: "error", message: String(err) },
      }));
    }
  }

  function handleSelectCommentFile(path: string) {
    updateTab(activeTabId,(t) => ({ ...t, selectedCommentFile: path }));
  }

  async function handleReply(threadId: string, commentId: string, body: string) {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest || tab.commentThreads.status !== "loaded") return;

    // Optimistic update: add a placeholder comment
    const optimisticComment = {
      id: `optimistic-${Date.now()}`,
      body,
      author: { login: "you", avatar_url: "" },
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
      url: "",
      reactions: [],
    };

    const prevThreads = tab.commentThreads.threads;
    const optimisticThreads = prevThreads.map((t) =>
      t.id === threadId
        ? { ...t, comments: [...t.comments, optimisticComment] }
        : t
    );
    updateTab(activeTabId,(t) => ({
      ...t,
      commentThreads: { status: "loaded", threads: optimisticThreads },
    }));

    try {
      const newComment = await invoke<ReviewComment>("reply_to_thread", {
        prUrl: tab.manifest.pr_url,
        commentId,
        body,
      });

      // Replace optimistic comment with real one
      setTabs((prev) =>
        prev.map((t) => {
          if (t.id !== activeTabId || t.commentThreads.status !== "loaded") return t;
          return {
            ...t,
            commentThreads: {
              status: "loaded",
              threads: t.commentThreads.threads.map((th) =>
                th.id === threadId
                  ? {
                      ...th,
                      comments: th.comments.map((c) =>
                        c.id === optimisticComment.id ? newComment : c
                      ),
                    }
                  : th
              ),
            },
          };
        })
      );
    } catch {
      // Revert on error
      updateTab(activeTabId,(t) => ({
        ...t,
        commentThreads: { status: "loaded", threads: prevThreads },
      }));
    }
  }

  async function handleToggleResolved(threadId: string, resolve: boolean) {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || tab.commentThreads.status !== "loaded") return;

    const prevThreads = tab.commentThreads.threads;

    // Optimistic update
    const optimisticThreads = prevThreads.map((t) =>
      t.id === threadId ? { ...t, is_resolved: resolve } : t
    );
    updateTab(activeTabId,(t) => ({
      ...t,
      commentThreads: { status: "loaded", threads: optimisticThreads },
    }));

    try {
      await invoke<boolean>("toggle_thread_resolved", { threadId, resolve });
    } catch {
      // Revert on error
      updateTab(activeTabId,(t) => ({
        ...t,
        commentThreads: { status: "loaded", threads: prevThreads },
      }));
    }
  }

  async function handleEditComment(commentId: string, body: string) {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || tab.commentThreads.status !== "loaded") return;

    const prevThreads = tab.commentThreads.threads;

    // Optimistic update
    const optimisticThreads = prevThreads.map((t) => ({
      ...t,
      comments: t.comments.map((c) =>
        c.id === commentId ? { ...c, body } : c
      ),
    }));
    updateTab(activeTabId,(t) => ({
      ...t,
      commentThreads: { status: "loaded" as const, threads: optimisticThreads },
    }));

    try {
      const updated = await invoke<ReviewComment>("update_review_comment", {
        commentId,
        body,
      });

      setTabs((prev) =>
        prev.map((t) => {
          if (t.id !== activeTabId || t.commentThreads.status !== "loaded") return t;
          return {
            ...t,
            commentThreads: {
              status: "loaded" as const,
              threads: t.commentThreads.threads.map((th) => ({
                ...th,
                comments: th.comments.map((c) =>
                  c.id === commentId ? updated : c
                ),
              })),
            },
          };
        })
      );
    } catch {
      updateTab(activeTabId,(t) => ({
        ...t,
        commentThreads: { status: "loaded" as const, threads: prevThreads },
      }));
    }
  }

  async function handleToggleReaction(commentId: string, content: string) {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || tab.commentThreads.status !== "loaded") return;

    const prevThreads = tab.commentThreads.threads;

    // Single pass: determine add vs remove and build optimistic update together
    let willAdd = true;
    const optimisticThreads = prevThreads.map((th) => {
      if (!th.comments.some((c) => c.id === commentId)) return th;
      return {
        ...th,
        comments: th.comments.map((c) => {
          if (c.id !== commentId) return c;
          const reactions = c.reactions ?? [];
          const existing = reactions.find((r) => r.content === content);
          const add = !(existing?.viewer_has_reacted);
          willAdd = add;
          if (add) {
            if (existing) {
              return { ...c, reactions: reactions.map((r) => r.content === content ? { ...r, total_count: r.total_count + 1, viewer_has_reacted: true } : r) };
            }
            return { ...c, reactions: [...reactions, { content, total_count: 1, viewer_has_reacted: true }] };
          }
          return {
            ...c,
            reactions: reactions
              .map((r) => r.content === content ? { ...r, total_count: r.total_count - 1, viewer_has_reacted: false } : r)
              .filter((r) => r.total_count > 0),
          };
        }),
      };
    });
    updateTab(activeTabId, (t) => ({
      ...t,
      commentThreads: { status: "loaded" as const, threads: optimisticThreads },
    }));

    try {
      await invoke("toggle_reaction", { commentId, content, add: willAdd });
    } catch {
      updateTab(activeTabId, (t) => ({
        ...t,
        commentThreads: { status: "loaded" as const, threads: prevThreads },
      }));
    }
  }

  async function handleSubmitReview(event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT", body: string) {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;

    try {
      await invoke<string>("submit_review", {
        prUrl: tab.manifest.pr_url,
        event,
        body,
      });

      // Update review state immediately (submitting clears the review request)
      const statusMap: Record<string, string> = {
        APPROVE: "approved",
        REQUEST_CHANGES: "changes_requested",
        COMMENT: "commented",
      };
      updateTab(activeTabId, (t) => ({
        ...t,
        myReviewState: {
          status: statusMap[event] as MyReviewState["status"],
          is_re_requested: false,
          is_merged: t.myReviewState?.is_merged ?? false,
        },
      }));

      // Re-fetch threads to update resolved states
      if (tab.commentThreads.status === "loaded") {
        const threads = await invoke<ReviewThread[]>("fetch_review_comments", {
          prUrl: tab.manifest.pr_url,
        });
        updateTab(activeTabId,(t) => ({
          ...t,
          commentThreads: { status: "loaded" as const, threads },
        }));
      }
    } catch (err) {
      setError(String(err));
    }
  }

  async function handleCreateComment(path: string, endLine: number, side: "LEFT" | "RIGHT", body: string, startLine?: number, startSide?: "LEFT" | "RIGHT") {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;

    try {
      const newThread = await invoke<ReviewThread>("create_review_comment", {
        prUrl: tab.manifest.pr_url,
        body,
        path,
        line: endLine,
        side,
        startLine: startLine ?? null,
        startSide: startSide ?? null,
      });

      // Add the new thread to the comment threads state
      updateTab(activeTabId,(t) => {
        if (t.commentThreads.status === "loaded") {
          return {
            ...t,
            commentThreads: {
              status: "loaded",
              threads: [...t.commentThreads.threads, newThread],
            },
          };
        }
        return {
          ...t,
          commentThreads: { status: "loaded", threads: [newThread] },
        };
      });
    } catch (err) {
      setError(String(err));
    }
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
  closeTabRef.current = closeTab;

  // Cmd+W / Cmd+T are owned by the native menu (see src-tauri/menu.rs) and arrive
  // as these events rather than keydowns — the menu intercepts them before the
  // webview's default window handling, which JS preventDefault couldn't reach.
  useEffect(() => {
    const unlistenClose = listen("menu-close-tab", () => {
      const id = activeTabIdRef.current;
      if (!id) return;
      const tab = tabsRef.current.find((t) => t.id === id);
      // On a review tab, skip Cmd+W while typing so it can't nuke a comment draft.
      // Opener tabs only hold a URL field, so allow Cmd+W there (it may quit the app).
      if (tab && !isOpenerTab(tab)) {
        const el = document.activeElement as HTMLElement | null;
        if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable)) return;
      }
      closeTabRef.current(id);
    });
    const unlistenNew = listen("menu-new-tab", () => newTabRef.current());
    // Chrome-style: first Cmd+Q arms a "press again to quit" hint; a second press
    // within 2s quits. Otherwise the hint fades and the arm resets.
    const unlistenQuit = listen("menu-quit-request", () => {
      if (quitArmedRef.current) {
        if (quitTimerRef.current !== null) clearTimeout(quitTimerRef.current);
        exit(0);
        return;
      }
      quitArmedRef.current = true;
      setShowQuitHint(true);
      if (quitTimerRef.current !== null) clearTimeout(quitTimerRef.current);
      quitTimerRef.current = window.setTimeout(() => {
        quitArmedRef.current = false;
        setShowQuitHint(false);
        quitTimerRef.current = null;
      }, 2000);
    });
    return () => {
      unlistenClose.then((fn) => fn());
      unlistenNew.then((fn) => fn());
      unlistenQuit.then((fn) => fn());
      if (quitTimerRef.current !== null) clearTimeout(quitTimerRef.current);
    };
  }, []);

  useEffect(() => {
    const interval = setInterval(async () => {
      const currentTabs = tabsRef.current;
      if (fetchingRef.current || currentTabs.length === 0) return;

      const pollableTabs = currentTabs.filter((t) => t.manifest && !t.isRefreshing);
      await Promise.allSettled(
        pollableTabs.map(async (tab) => {
          const status = await invoke<PrUpdateStatus>("check_pr_updates", {
            prUrl: tab.manifest!.pr_url,
            currentHeadSha: tab.manifest!.head_sha,
            currentCommentCount: tab.lastCommentCount ?? 0,
          });

          if (!status.has_changes) return;

          if (status.head_sha_changed) {
            handleRefreshPr(tab.id);
          } else if (status.comment_count_changed) {
            handleRefreshComments(tab.id);
          }
        })
      );
    }, 60_000);

    return () => clearInterval(interval);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const interval = setInterval(async () => {
      const currentTabs = tabsRef.current;
      if (fetchingRef.current || currentTabs.length === 0) return;

      const pollableTabs = currentTabs.filter((tab) => {
        if (!tab.manifest) return false;
        const existing = checksMapRef.current[tab.id];
        if (existing && existing.overall_state === "success") return false;
        if (checksDismissedRef.current[tab.manifest.pr_url]) return false;
        return true;
      });

      await Promise.allSettled(
        pollableTabs.map(async (tab) => {
          try {
            const checks = await invoke<PrChecksStatus>("get_pr_checks", {
              prUrl: tab.manifest!.pr_url,
            });
            setChecksMap((prev) => {
              const existing = prev[tab.id];
              if (existing && existing.overall_state === checks.overall_state) return prev;
              return { ...prev, [tab.id]: checks };
            });
          } catch {
            // Poll will retry
          }
        })
      );
    }, 30_000);

    return () => clearInterval(interval);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Re-load dismissed-highlight state from disk when the window regains focus, so
  // dismissals made outside the app (e.g. an external/AI tool writing the
  // ~/.config/marrow/dismissed file) show up without reopening the PR.
  useEffect(() => {
    const unlisten = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused) return;
      for (const tab of tabsRef.current) {
        if (!tab.manifest) continue;
        const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
        invoke<{ keys: string[] } | null>("load_dismissed_highlights", { owner, repo, prNumber: number })
          .then((saved) => {
            const keys = saved?.keys ?? [];
            updateTab(tab.id, (t) => {
              const same = t.dismissedHighlights.size === keys.length && keys.every((k) => t.dismissedHighlights.has(k));
              return same ? t : { ...t, dismissedHighlights: new Set(keys) };
            });
          })
          .catch(() => {});
      }
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  // Persist session state whenever tabs or active tab change (debounced)
  useEffect(() => {
    if (!sessionRestoredRef.current) return;
    const timer = setTimeout(() => {
      const state: SessionState = {
        open_prs: tabs
          .filter((t) => t.manifest)
          .map((t) => ({
            pr_url: t.manifest!.pr_url,
            selected_file: t.selectedFile?.path ?? null,
            sidebar_view: t.sidebarView,
            selected_comment_file: t.selectedCommentFile,
          })),
        active_pr: tabs.find((t) => t.id === activeTabId)?.manifest?.pr_url ?? null,
      };
      invoke("save_session", { state }).catch(() => {});
    }, 500);
    return () => clearTimeout(timer);
  }, [tabs, activeTabId]);

  // Persist user preferences whenever they change (debounced, with dirty check)
  useEffect(() => {
    const s = settingsRef.current;
    if (!s) return;
    if (s.view_mode === viewMode && s.show_hunk_significance === showHunkSignificance
        && s.show_ai_notes === showAiNotes && s.hunk_filter === hunkFilter) return;
    const timer = setTimeout(() => {
      const updated = { ...settingsRef.current!, view_mode: viewMode, show_hunk_significance: showHunkSignificance, show_ai_notes: showAiNotes, hunk_filter: hunkFilter };
      settingsRef.current = updated;
      invoke("save_settings", { settings: updated }).catch(() => {});
    }, 500);
    return () => clearTimeout(timer);
  }, [viewMode, showHunkSignificance, showAiNotes, hunkFilter]);

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

  const overlays = (
    <>
      <UpdateBanner
        status={updateStatus}
        onDownload={handleDownloadUpdate}
        onRelaunch={relaunch}
        onDismiss={() => setUpdateStatus({ state: "idle" })}
      />
      {helpOpen && <KeyboardHelp onClose={() => setHelpOpen(false)} />}
      {reviewPickerOpen && (
        <ReviewPicker
          onClose={() => setReviewPickerOpen(false)}
          onSubmit={(event, body) => { handleSubmitReview(event, body); setReviewPickerOpen(false); }}
        />
      )}
      <ToastContainer toasts={toasts} onDismiss={removeToast} />
      <div
        className={`quit-hint${showQuitHint ? " visible" : ""}`}
        role="status"
        aria-live="polite"
        aria-hidden={!showQuitHint}
      >
        Press <kbd>⌘Q</kbd> again to quit
      </div>
    </>
  );

  if (error) {
    return (
      <div className="app error-state">
        <div className="error-message">
          <h2>Error loading review</h2>
          <pre>{error}</pre>
          <button
            className="settings-button"
            onClick={() => {
              setError(null);
            }}
            style={{ marginTop: 16 }}
          >
            Back
          </button>
        </div>
        {overlays}
      </div>
    );
  }

  return (
    <div className="app">
      <ActivityWidget onOpenPr={(ref) => handleFetchStart(ref, activeTabId ?? undefined)} />
      <Header
        tabs={tabs}
        activeTabId={activeTabId}
        onSelectTab={handleSelectTab}
        onCloseTab={closeTab}
        onNewReview={handleNewReview}
        viewMode={viewMode}
        onViewModeChange={setViewMode}
        viewedCount={activeTab?.viewedFiles.size ?? 0}
        staleCount={activeTab?.staleViewedFiles.size ?? 0}
        onSettingsClick={() => setSettingsOpen(true)}
        manifest={activeTab?.manifest ?? null}
        showHunkSignificance={showHunkSignificance}
        onToggleHunkSignificance={() => setShowHunkSignificance((v) => !v)}
        showAiNotes={showAiNotes}
        onToggleAiNotes={() => setShowAiNotes((v) => !v)}
        commentThreads={activeTab?.commentThreads}
        onSubmitReview={activeTab ? handleSubmitReview : undefined}
        onRefresh={activeTab ? () => handleRefreshPr() : undefined}
        isRefreshing={activeTab?.isRefreshing}
        myReviewState={activeTab?.myReviewState}
        checksBlocking={showChecksModal}
        onCheckForUpdates={() => checkForUpdates(false)}
      />
      <SettingsModal
        open={settingsOpen}
        onClose={handleSettingsClose}
      />
      {!activeTab ? null : activeTab.manifest === null ? (
        <div
          className="opener-tab"
          onDragOver={(e) => e.preventDefault()}
          onDrop={handleFileDrop}
        >
          <div className="empty-message">
            {activeTab.loading ? (
              <LoadingView
                prRef={activeTab.loading.prRef}
                prTitle={activeTab.loading.prTitle}
                progress={activeTab.loading.progress}
                fileCounts={activeTab.loading.fileCounts}
                onCancel={() => handleFetchCancel(activeTab.id)}
              />
            ) : (
              <>
                {activeTab.error && (
                  <div className="opener-error" role="alert">
                    <strong>Failed to load PR</strong>
                    <pre>{activeTab.error}</pre>
                  </div>
                )}
                <h1>Marrow</h1>
                <p>
                  Drop a manifest JSON file here, or enter a PR URL below to start
                  a review.
                </p>
                <PrOpener
                  onFetchStart={(ref) => handleFetchStart(ref, activeTab.id)}
                  onSettingsClick={() => setSettingsOpen(true)}
                  onCheckForUpdates={() => checkForUpdates(false)}
                />
                <ReviewRequestList
                  onSelectPr={(ref) => handleFetchStart(ref, activeTab.id)}
                  openPrUrls={openPrUrls}
                />
              </>
            )}
          </div>
        </div>
      ) : (
        <div className="review-content">
        {showChecksModal && (
          <ChecksBlockingModal
            checksStatus={activeChecks!}
            onDismiss={() => handleDismissChecks(activeTab.manifest!.pr_url)}
          />
        )}
        <SearchBar
          ref={searchRef}
          files={activeTab.manifest.files}
          selectedFile={activeTab.selectedFile}
          onSelectFile={setSelectedFile}
          onHighlightMatches={(matches, idx, q) => { setSearchMatches(matches); setSearchCurrentIndex(idx); setSearchQuery(q); }}
          onClearHighlights={() => { setSearchMatches([]); setSearchCurrentIndex(0); setSearchQuery(""); }}
          onOpenChange={setSearchOpen}
        />
        <div className="main-content">
          <FileSidebar
            files={activeTab.manifest.files}
            changeGroups={activeTab.manifest.change_groups ?? []}
            selectedFile={activeTab.selectedFile}
            onSelectFile={setSelectedFile}
            viewedFiles={activeTab.viewedFiles}
            staleViewedFiles={activeTab.staleViewedFiles}
            onToggleViewed={toggleViewed}
            showHunkSignificance={showHunkSignificance}
            hunkFilter={hunkFilter}
            onHunkFilterChange={setHunkFilter}
            sidebarView={activeTab.sidebarView}
            onViewChange={handleViewChange}
            commentThreads={activeTab.commentThreads.status === "loaded" ? activeTab.commentThreads.threads : []}
            selectedCommentFile={activeTab.selectedCommentFile}
            onSelectCommentFile={handleSelectCommentFile}
            onVisibleFilesChange={handleVisibleFilesChange}
          />
          <div className="diff-pane">
            {activeTab.sidebarView === "comments" ? (
              activeTab.commentThreads.status === "loading" ? (
                <div className="no-file-selected">Loading review threads...</div>
              ) : activeTab.commentThreads.status === "error" ? (
                <div className="no-file-selected" style={{ color: "var(--diff-remove-text)" }}>
                  {activeTab.commentThreads.message}
                </div>
              ) : activeTab.commentThreads.status === "loaded" ? (
                <CommentsViewer
                  threads={activeTab.commentThreads.threads}
                  selectedFile={activeTab.selectedCommentFile}
                  onReply={handleReply}
                  onToggleResolved={handleToggleResolved}
                  onEditComment={handleEditComment}
                  onToggleReaction={handleToggleReaction}
                  lang={activeTab.selectedCommentFile ? detectLanguage(activeTab.selectedCommentFile) : undefined}
                />
              ) : (
                <div className="no-file-selected">Switch to Comments tab to load threads</div>
              )
            ) : activeTab.selectedFile ? (
              <DiffViewer ref={diffViewerRef} key={activeTab.selectedFile.path} file={activeTab.selectedFile} viewMode={viewMode} showHunkSignificance={showHunkSignificance} showAiNotes={showAiNotes} dismissedHighlights={activeTab.dismissedHighlights} onToggleHighlightDismissed={toggleHighlightDismissed} onCreateComment={handleCreateComment} onEditComment={handleEditComment} onReply={handleReply} onToggleResolved={handleToggleResolved} onToggleReaction={handleToggleReaction} reviewThreads={activeTab.commentThreads.status === "loaded" ? activeTab.commentThreads.threads : undefined} searchMatches={fileSearchMatches} currentSearchMatch={currentSearchMatch} searchQuery={searchQuery} />
            ) : activeTab.manifest.summary ? (
              <div className="pr-summary">
                <h3>PR Summary</h3>
                <SummaryParagraphs text={activeTab.manifest.summary} />
              </div>
            ) : (
              <div className="no-file-selected">Select a file to review</div>
            )}
          </div>
        </div>
        </div>
      )}
      {overlays}
    </div>
  );
}

export default App;
