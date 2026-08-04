import { useState, useEffect, useRef, useMemo, useCallback } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";
import { FileSidebar } from "./components/FileSidebar";
import { DiffViewer, type DiffViewerHandle } from "./components/DiffViewer";
import { CommentsPanel } from "./components/CommentsPanel";
import { Header } from "./components/Header";
import { PrOpener } from "./components/PrOpener";
import { ReviewRequestList } from "./components/ReviewRequestList";
import { ActivityWidget } from "./components/ActivityWidget";
import { LoadingView } from "./components/LoadingView";
import { SettingsModal } from "./components/SettingsModal";
import { ChecksBlockingModal } from "./components/ChecksBlockingModal";
import { PrOverview } from "./components/PrOverview";
import { CommitsLens } from "./components/CommitsLens";
import { NextFileBar } from "./components/NextFileBar";
import { SearchBar, type SearchBarHandle } from "./components/SearchBar";
import { KeyboardHelp } from "./components/KeyboardHelp";
import { ReviewPicker } from "./components/ReviewPicker";
import { ToastContainer, createToast, type ToastData } from "./components/Toast";
import { CommandPalette, type PaletteCommand } from "./components/CommandPalette";
import { WelcomeSetup } from "./components/WelcomeSetup";
import { ChatPanel } from "./components/ChatPanel";
import { parseChatActionFences } from "./components/RichText";
import { open as openUrl } from "@tauri-apps/plugin-shell";
import { UpdateBanner } from "./components/UpdateBanner";
import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch, exit } from "@tauri-apps/plugin-process";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { ReviewManifest, FileDiff, DiffViewMode, Tab, FetchProgress, HunkSignificanceFilter, SidebarView, ReviewThread, ReviewComment, SearchMatch, PrUpdateStatus, ViewedFileState, MyReviewState, PrChecksStatus, UpdateStatus, SessionState, Settings, CachedPrInfo, ChatState, ChatMessage, ChatStreamEvent, ChatAction, NoteResolution, PrCommit, CommitDiff, CheckAnnotation, CheckFailures, PrLens, ChangeGroup } from "./types";
import { parsePrUrl, extractPrRef, canonicalPrKey, highlightKey } from "./utils";
import type { ReviewSession } from "./hooks/useActivityFeed";

/** An empty "open a PR" tab — no loaded PR, not mid-fetch, no error. */
function isOpenerTab(tab: Tab): boolean {
  return !tab.manifest && !tab.loading && !tab.error;
}

/** Prompt sent by the "Brief me" command — a whole-PR guided walkthrough.
 * Deliberately strict about brevity: without hard limits the model produces an
 * exhausting per-change essay instead of a scannable briefing. */
const BRIEF_ME_PROMPT =
  "Brief me on this PR — a briefing I can scan in under a minute, not an essay. " +
  "Start with a one-sentence TL;DR. Then one line per change, most important first: " +
  "a `file:line` citation (in backticks, so I can jump there), what it does, and its sharpest risk if it has one. " +
  "Hard limits: at most 7 lines, roughly 25 words each, no sub-bullets, no headings, no code snippets, no restating the diff. " +
  "Merge related changes into one line. Skip filler like 'low risk' or 'mechanical change' — silence means fine. " +
  "End with one line: where to spend my review time. If I want depth on a stop, I'll ask.";

/** Ceiling on marrow-action blocks auto-executed per streaming turn — the
 * backstop behind the prompt's "at most a few actions per reply". */
const MAX_AUTO_ACTIONS_PER_TURN = 6;

/** A fresh, closed chat panel for a new tab. */
function emptyChatState(): ChatState {
  return { messages: [], status: "idle", streamingText: "", streamingStatus: null, includeWholePr: false, open: false };
}

/** The repo's base GitHub URL (e.g. `https://github.com/owner/repo`), derived
 * by stripping the `/pull/<n>` suffix off a PR URL — used to build commit URLs. */
function repoBaseUrl(prUrl: string): string {
  return prUrl.replace(/\/pull\/\d+\/?$/, "");
}

/** Every AI highlight key (see highlightKey) across a manifest's files. */
function collectHighlightKeys(manifest: ReviewManifest): Set<string> {
  const keys = new Set<string>();
  for (const f of manifest.files) {
    for (const h of f.highlights) keys.add(highlightKey(f.path, h));
  }
  return keys;
}

function App() {
  const nextTabId = useRef(1);
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeTabId, setActiveTabId] = useState<string | null>(null);
  const [viewMode, setViewMode] = useState<DiffViewMode>("split");
  const [showHunkSignificance, setShowHunkSignificance] = useState(true);
  const [showAiNotes, setShowAiNotes] = useState(true);
  const [hunkFilter, setHunkFilter] = useState<HunkSignificanceFilter>("all");
  const [expandAllHunks, setExpandAllHunks] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  // Commit scope (issue #147, per-tab as of #170): which commit the Commits
  // lens is showing lives on the tab itself (tab.selectedCommit) so it can't
  // leak across tabs. The fetched diff, its loading/error state, and the
  // session-only cache (never invalidated — commit diffs are immutable) keyed
  // by sha stay App-level, shared across tabs.
  const [commitDiffLoading, setCommitDiffLoading] = useState(false);
  const [commitDiffError, setCommitDiffError] = useState<string | null>(null);
  const [commitDiff, setCommitDiff] = useState<CommitDiff | null>(null);
  const commitDiffCacheRef = useRef(new Map<string, CommitDiff>());
  // In-flight `${tabId}:${sha}` fetches, so the resync effect (on tab switch)
  // and an interactive commit click never both issue the same request.
  const commitDiffFetchingRef = useRef(new Set<string>());
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [welcomeOpen, setWelcomeOpen] = useState(false);
  // Cached PR whose head moved — re-analyzing costs an AI pass, so confirm.
  const [staleConfirm, setStaleConfirm] = useState<{ prRef: string; title: string } | null>(null);
  const [reviewPickerOpen, setReviewPickerOpen] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const [queueFilter, setQueueFilter] = useState("");
  const [viewerLogin, setViewerLogin] = useState<string | null>(null);
  const searchRef = useRef<SearchBarHandle>(null);
  const diffViewerRef = useRef<DiffViewerHandle>(null);
  // Jump-to-thread from the comments panel: the target thread id, and a ping
  // bumped on every jump so the retry effect below reruns even when the
  // selected file doesn't change (thread already on the open file).
  const pendingThreadIdRef = useRef<string | null>(null);
  const [threadScrollPing, setThreadScrollPing] = useState(0);
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
  // Chat ```marrow-action chip statuses (issue #166): tabId -> message key
  // ("msg-<index>" for a finalized message, "streaming" for the in-progress
  // turn) -> `${blockIndex}:${JSON.stringify(action)}` -> outcome. Session-
  // only — never persisted alongside chat history.
  const [chatActionStatuses, setChatActionStatuses] = useState<Record<string, Record<string, Record<string, "done" | "failed">>>>({});
  // Per-tab set of action-block keys already auto-executed during the current
  // streaming turn, so a block that already ran isn't re-run on the next delta.
  const chatExecutedActionsRef = useRef<Record<string, Set<string>>>({});
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
    invoke<Settings>("get_settings").then((s) => {
      settingsRef.current = s;
      setExpandAllHunks(s.expand_all_hunks ?? false);
    }).catch(() => {});
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

  // commitDiff/commitDiffLoading/commitDiffError are single App-level slots
  // (not per-tab — see the state declarations above), so switching tabs must
  // resync them to whatever the newly-active tab's Commits lens should show:
  // serve the cached diff, kick off a fetch on a cache miss, or clear the
  // slots so a later entry into the lens starts clean. Also fires when the
  // active tab's own lens/selectedCommit changes (entering/leaving Commits,
  // or picking a different commit) so it stays one code path with clicks.
  useEffect(() => {
    if (!activeTab || activeTab.lens !== "commits" || !activeTab.selectedCommit) {
      setCommitDiff(null);
      setCommitDiffLoading(false);
      setCommitDiffError(null);
      return;
    }
    loadCommitDiff(activeTab.id, activeTab.selectedCommit);
  }, [activeTabId, activeTab?.lens, activeTab?.selectedCommit]); // eslint-disable-line react-hooks/exhaustive-deps

  // Lens switcher segment counts (issue #170). Files count mirrors the
  // relevant/fallback-to-total rule buildChatFiles uses for whole-PR chat scope.
  const relevantFileCount = activeTab?.manifest?.files.filter((f) => f.classification === "RELEVANT").length ?? 0;
  const filesLensCount = activeTab?.manifest ? (relevantFileCount > 0 ? relevantFileCount : activeTab.manifest.files.length) : 0;
  const commitsLensCount = activeTab?.manifest?.commits.length ?? 0;

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

  // Inline CI failure annotations for the active tab, once loaded (see the
  // fetch effect above) — grouped by path so the sidebar badge and the diff
  // pane's inline markers are cheap lookups.
  const annotationsByPath = useMemo(() => {
    if (activeTab?.checkAnnotations.status !== "loaded") return null;
    const map = new Map<string, CheckAnnotation[]>();
    for (const a of activeTab.checkAnnotations.failures.annotations) {
      const arr = map.get(a.path);
      if (arr) arr.push(a); else map.set(a.path, [a]);
    }
    return map;
  }, [activeTab?.checkAnnotations]);
  const checkFailureCounts = useMemo(() => {
    const map = new Map<string, number>();
    if (annotationsByPath) {
      for (const [path, anns] of annotationsByPath) map.set(path, anns.length);
    }
    return map;
  }, [annotationsByPath]);
  const selectedFileAnnotations = (selectedFilePath && annotationsByPath?.get(selectedFilePath)) || [];
  const activeCheckFailures = activeTab?.checkAnnotations.status === "loaded" ? activeTab.checkAnnotations.failures : null;

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
    if (!activeTab.viewedFiles.has(path)) toggleViewed(path);
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

  function selectAdjacentTab(delta: 1 | -1) {
    if (tabs.length < 2) return;
    const i = tabs.findIndex((t) => t.id === activeTabId);
    if (i < 0) return;
    handleSelectTab(tabs[(i + delta + tabs.length) % tabs.length].id);
  }

  // Chat and Comments are mutually exclusive right-dock panels — opening one closes the other.
  function toggleThreadsView() {
    if (!activeTab?.manifest) return;
    const opening = !activeTab.commentsOpen;
    updateTab(activeTabId, (t) => ({
      ...t,
      commentsOpen: opening,
      chat: opening ? { ...t.chat, open: false } : t.chat,
    }));
    // Fetch is driven by the commentsOpen+idle effect below, so every path
    // that opens the panel (toggle, palette, legacy session restore) fetches.
  }

  // Whenever the panel is open on a tab whose threads were never fetched,
  // fetch them. Single trigger for all open paths — including a restored
  // pre-panel session that had the old comments *mode* persisted.
  useEffect(() => {
    if (activeTab?.commentsOpen && activeTab.manifest && activeTab.commentThreads.status === "idle") {
      handleRequestComments();
    }
  }, [activeTabId, activeTab?.commentsOpen, activeTab?.commentThreads.status]); // eslint-disable-line react-hooks/exhaustive-deps

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
      onCloseOverlays: () => { setHelpOpen(false); setReviewPickerOpen(false); setPaletteOpen(false); },
      // Not during first-run setup — the palette would open invisibly under
      // the welcome card and pop up when setup closes.
      onTogglePalette: () => { if (!welcomeOpen) setPaletteOpen((v) => !v); },
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
      onToggleChat: () => { if (!welcomeOpen) toggleChatOpen(); },
      onSetLens: (lens) => { if (activeTabId) setLens(activeTabId, lens); },
    },
    {
      enabled: !!activeTab?.manifest,
      overlayOpen: helpOpen || settingsOpen || searchOpen || showChecksModal || reviewPickerOpen || paletteOpen,
    },
  );

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
      chat: emptyChatState(),
      commentThreads: { status: "idle" },
      checkAnnotations: { status: "idle" },
      commentsOpen: false,
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
      chat: emptyChatState(),
      commentThreads: { status: "idle" },
      checkAnnotations: { status: "idle" },
      commentsOpen: false,
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
        setExpandAllHunks(settings.expand_all_hunks ?? false);
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
                  // Pre-#144 sessions may have persisted the now-removed "comments"
                  // mode — map it onto the panel instead of a dead sidebar view.
                  if ((entry.sidebar_view as string) === "comments") {
                    const hasGroups = (manifest.change_groups ?? []).length > 0;
                    tab.sidebarView = hasGroups ? "groups" : "category";
                    tab.commentsOpen = true;
                  } else {
                    tab.sidebarView = entry.sidebar_view as SidebarView;
                  }
                }
                // A session written before lenses existed (or a malformed
                // value) falls back to "overview" rather than failing restore.
                if (entry.lens === "files" || entry.lens === "commits") {
                  tab.lens = entry.lens;
                }
                // Restoring straight into the Files lens without a selected
                // file (no persisted selection, or it no longer exists) skips
                // setLens entirely — auto-select guided-first here the same
                // way, scoped to THIS tab rather than activeTab (guidedOrder/
                // nextUnviewed both accept an explicit tab for exactly this).
                if (tab.lens === "files" && !tab.selectedFile) {
                  const order = guidedOrder(tab);
                  const firstPath = nextUnviewed(order, -1, undefined, tab)?.path ?? order[0];
                  const first = firstPath ? manifest.files.find((f) => f.path === firstPath) : undefined;
                  if (first) tab.selectedFile = first;
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
              loadChatHistory(tab);
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

      // First run (no token anywhere, never completed/skipped setup) shows the
      // two-step welcome instead of a blank queue + hidden settings.
      invoke<boolean>("needs_setup")
        .then((needed) => { if (needed) setWelcomeOpen(true); })
        .catch(() => {});
      invoke<string>("get_viewer_login")
        .then(setViewerLogin)
        .catch(() => {});
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

  /** Bumped per resume event so the advance effect runs even without a tab-id transition. */
  const [resumePing, setResumePing] = useState(0);

  // Listen for "resume this PR" from the mini-player's Now Reviewing card. If
  // a tab for it is already open, jump back in and advance past whatever was
  // last viewed; otherwise fall back to the ordinary deep-link open flow.
  useEffect(() => {
    const unlisten = listen<string>("deep-link-resume", (event) => {
      if (!event.payload) return;
      // canonicalPrKey (unlike extractPrRef) also accepts the owner/repo#number
      // shape this event carries, not just a github.com URL.
      const incomingKey = canonicalPrKey(event.payload);
      const existing = incomingKey
        ? tabsRef.current.find(
            (t) => t.manifest && canonicalPrKey(t.manifest.pr_url) === incomingKey
          )
        : null;
      if (existing) {
        pendingResumeTabIdRef.current = existing.id;
        // Bump a nonce so the advance effect below runs even when the target
        // tab is ALREADY active — handleSelectTab with the current id causes
        // no state transition, and without a run the stale ref would fire on
        // a later unrelated switch back to this tab, yanking the user's file.
        setResumePing((p) => p + 1);
        handleSelectTab(existing.id);
        return;
      }
      if (fetchingRef.current) {
        addToast("info", "Already fetching a PR — try again when it finishes.");
        return;
      }
      handleFetchStart(event.payload);
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Once the tab targeted by `deep-link-resume` above actually becomes active
  // (a render after handleSelectTab), advance to the next unviewed file —
  // deferred to here because guidedOrder()/nextUnviewed() close over this
  // render's `activeTab`, which isn't current yet inside the listener above.
  useEffect(() => {
    if (!pendingResumeTabIdRef.current) return;
    if (activeTabId !== pendingResumeTabIdRef.current) return;
    if (!activeTab?.manifest) return;
    pendingResumeTabIdRef.current = null;
    const next = nextUnviewed(guidedOrder(), -1);
    if (next) setSelectedFile(next);
  }, [activeTabId, activeTab?.manifest, resumePing]); // eslint-disable-line react-hooks/exhaustive-deps

  // Once a jump-to-thread target's file is selected, the DiffViewer for it may
  // not have mounted (or rendered the thread row) yet on this same tick — retry
  // via rAF for up to ~1s rather than a single synchronous call.
  useEffect(() => {
    if (!pendingThreadIdRef.current) return;
    const id = pendingThreadIdRef.current;
    let attempts = 0;
    let raf = 0;
    const tryScroll = () => {
      attempts++;
      // First attempt may expand collapsed low-significance hunks — a thread
      // inside one has no DOM row until its hunk renders.
      if (diffViewerRef.current?.scrollToThread(id, attempts === 1)) {
        pendingThreadIdRef.current = null;
        return;
      }
      if (attempts > 60) {
        pendingThreadIdRef.current = null;
        // Outdated threads (line: null, position gone from the current diff)
        // have no row to land on — say so instead of silently doing nothing.
        addToast("info", "Couldn't locate this thread in the current diff — it may be outdated.");
        return;
      }
      raf = requestAnimationFrame(tryScroll);
    };
    raf = requestAnimationFrame(tryScroll);
    return () => cancelAnimationFrame(raf);
  }, [activeTabId, activeTab?.selectedFile?.path, threadScrollPing]); // eslint-disable-line react-hooks/exhaustive-deps

  // Bridge for the mini-player's "Open on GitHub" row action: the floating
  // widget's webview has no shell:open capability (see mini-player.json), so
  // it can't call the shell plugin directly like the dock (which runs in this
  // same main window) does. It emits this event instead and the main window —
  // which does have the capability — opens the URL on its behalf.
  useEffect(() => {
    const unlisten = listen<string>("aw-open-external", (event) => {
      // Scope the bridge to what it exists for: activity items carry GitHub
      // html_urls, so anything else is a bug (or a compromised webview) and
      // gets dropped rather than handed to the OS opener.
      if (event.payload?.startsWith("https://github.com/")) {
        openUrl(event.payload).catch(() => {});
      }
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

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
    loadChatHistory(tab);
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
    loadChatHistory(tab);
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
      const saved = await invoke<{ keys: string[]; resolutions?: Record<string, NoteResolution> } | null>("load_dismissed_highlights", { owner, repo, prNumber: number });
      if (saved && saved.keys.length > 0) {
        updateTab(tab.id, (t) => ({
          ...t,
          dismissedHighlights: new Set(saved.keys),
          noteResolutions: new Map(Object.entries(saved.resolutions ?? {})),
        }));
      }
    } catch {
      // Non-critical: start with nothing dismissed on failure
    }
  }

  /** Persist the current tab's dismissed-set + resolutions in one write. */
  function saveDismissedState(tab: Tab, keys: Set<string>, resolutions: Map<string, NoteResolution>) {
    if (!tab.manifest) return;
    const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
    invoke("save_dismissed_highlights", {
      owner,
      repo,
      prNumber: number,
      state: { keys: [...keys], resolutions: Object.fromEntries(resolutions) },
    }).catch(() => addToast("error", "Couldn't save — this dismissal may not persist"));
  }

  /** Dismiss (hide) a note, optionally recording how/why it was resolved.
   * `resolution: null` is a plain/quick dismiss — no resolution metadata is
   * recorded (renders as the legacy "Dismissed" chip, same as noise). */
  function resolveHighlight(key: string, resolution: NoteResolution | null) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    const nextKeys = new Set(tab.dismissedHighlights);
    nextKeys.add(key);
    const nextResolutions = new Map(tab.noteResolutions);
    if (resolution) {
      nextResolutions.set(key, { ...resolution, at: new Date().toISOString() });
    } else {
      nextResolutions.delete(key);
    }
    updateTab(tab.id, (t) => ({ ...t, dismissedHighlights: nextKeys, noteResolutions: nextResolutions }));
    saveDismissedState(tab, nextKeys, nextResolutions);
  }

  /** Restore a previously-dismissed note, clearing any recorded resolution. */
  function restoreHighlight(key: string) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    const nextKeys = new Set(tab.dismissedHighlights);
    nextKeys.delete(key);
    const nextResolutions = new Map(tab.noteResolutions);
    nextResolutions.delete(key);
    updateTab(tab.id, (t) => ({ ...t, dismissedHighlights: nextKeys, noteResolutions: nextResolutions }));
    saveDismissedState(tab, nextKeys, nextResolutions);
  }

  async function loadChatHistory(tab: Tab) {
    if (!tab.manifest) return;
    try {
      const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
      const saved = await invoke<{ messages: ChatMessage[] } | null>("load_chat_history", { owner, repo, prNumber: number });
      if (saved && saved.messages.length > 0) {
        updateTab(tab.id, (t) => ({ ...t, chat: { ...t.chat, messages: saved.messages } }));
      }
    } catch {
      // Non-critical: start with an empty conversation on failure
    }
  }

  // ---- Conversational diff Q&A (chat) ----

  // Per-tab flag: when true, in-flight stream events for that tab are ignored
  // (the user pressed Stop, cleared the chat, or sent a fresh message).
  const chatCancelRef = useRef<Record<string, boolean>>({});
  // Per-tab id of the in-flight chat request, so Stop can abort it server-side.
  const chatRequestIdRef = useRef<Record<string, string>>({});

  /** The diff/content context for the chat. Effective scope is the whole PR
   * (relevant files only) when `includeWholePr` is set OR no file is selected
   * (the overview) — auto-scope means there's no "select a file first" error
   * path. Whole-PR omits full contents to save budget. AI highlights ride
   * along so questions about "the warning on L287-318" resolve against them. */
  function buildChatFiles(tab: Tab): Array<{ path: string; unified_diff: string; head_content?: string; highlights: FileDiff["highlights"] }> {
    const manifest = tab.manifest!;
    if (tab.chat.includeWholePr || !tab.selectedFile) {
      const relevant = manifest.files.filter((f) => f.classification === "RELEVANT");
      const files = relevant.length > 0 ? relevant : manifest.files;
      return files.map((f) => ({ path: f.path, unified_diff: f.unified_diff, highlights: f.highlights }));
    }
    const f = tab.selectedFile;
    return [{ path: f.path, unified_diff: f.unified_diff, head_content: f.head_content, highlights: f.highlights }];
  }

  /** Append the assistant's answer, return the chat to idle, and persist. */
  function finalizeChat(tabId: string, prUrl: string, content: string) {
    const tab = tabsRef.current.find((t) => t.id === tabId);
    const messages: ChatMessage[] = [...(tab?.chat.messages ?? []), { role: "assistant", content }];
    updateTab(tabId, (t) => ({ ...t, chat: { ...t.chat, messages, status: "idle", streamingText: "", streamingStatus: null } }));
    // The just-finished turn's action statuses were recorded under the
    // "streaming" bucket — move them to this message's own key (its index in
    // the now-final array) so its chips keep showing ✓/✗ after finalize.
    const msgKey = `msg-${messages.length - 1}`;
    setChatActionStatuses((prev) => {
      const streaming = prev[tabId]?.streaming;
      if (!streaming) return prev;
      const nextForTab = { ...prev[tabId] };
      delete nextForTab.streaming;
      nextForTab[msgKey] = streaming;
      return { ...prev, [tabId]: nextForTab };
    });
    try {
      const { owner, repo, number } = parsePrUrl(prUrl);
      invoke("save_chat_history", { owner, repo, prNumber: number, state: { messages } }).catch(() => {});
    } catch {
      // Non-critical: an unparseable URL just means this turn isn't persisted.
    }
  }

  function handleChatSend(message: string) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    const tabId = tab.id;
    const manifest = tab.manifest;
    const files = buildChatFiles(tab);
    if (files.length === 0) return;
    const userMsg: ChatMessage = {
      role: "user",
      content: message,
      filePath: tab.chat.includeWholePr ? undefined : tab.selectedFile?.path,
    };
    // Cap the history sent to the model — full history still renders and persists.
    const history = tab.chat.messages.slice(-12).map((m) => ({ role: m.role, content: m.content }));
    const requestId = crypto.randomUUID();

    chatCancelRef.current[tabId] = false;
    chatRequestIdRef.current[tabId] = requestId;
    // A fresh turn starts a fresh action-block execution window.
    chatExecutedActionsRef.current[tabId] = new Set();
    setChatActionStatuses((prev) => ({ ...prev, [tabId]: { ...prev[tabId], streaming: {} } }));
    updateTab(tabId, (t) => ({
      ...t,
      chat: { ...t.chat, messages: [...t.chat.messages, userMsg], status: "streaming", streamingText: "", streamingStatus: null, error: undefined },
    }));

    const channel = new Channel<ChatStreamEvent>();
    channel.onmessage = (ev) => {
      // Drop events from a cancelled request AND from a superseded one: after
      // Stop → new send, stragglers from the old stream can still arrive
      // (chat_cancel is fire-and-forget) and must not touch the new request.
      if (chatCancelRef.current[tabId] || chatRequestIdRef.current[tabId] !== requestId) return;
      if (ev.type === "delta") {
        // Any text clears a pending "Working…" status.
        updateTab(tabId, (t) => ({ ...t, chat: { ...t.chat, streamingText: t.chat.streamingText + ev.text, streamingStatus: null } }));
      } else if (ev.type === "status") {
        updateTab(tabId, (t) => ({ ...t, chat: { ...t.chat, streamingStatus: ev.label } }));
      } else if (ev.type === "done") {
        finalizeChat(tabId, manifest.pr_url, ev.content);
      } else if (ev.type === "error") {
        updateTab(tabId, (t) => ({ ...t, chat: { ...t.chat, status: "idle", streamingText: "", streamingStatus: null, error: ev.message } }));
      }
    };

    invoke("chat_send", {
      channel,
      request: {
        request_id: requestId,
        context: { pr_title: manifest.pr_title, summary: manifest.summary, files },
        history,
        message,
        // Repo identity for the read-only repo tools (issue #150); absent
        // (rather than throwing) falls back to diff-only chat server-side.
        repo: (() => {
          try {
            const { owner, repo } = parsePrUrl(manifest.pr_url);
            return { owner, repo, head_sha: manifest.head_sha };
          } catch {
            return undefined;
          }
        })(),
      },
    }).catch((err) => {
      if (chatCancelRef.current[tabId] || chatRequestIdRef.current[tabId] !== requestId) return;
      updateTab(tabId, (t) => ({ ...t, chat: { ...t.chat, status: "idle", streamingText: "", error: String(err) } }));
    });
  }

  /** Abort the in-flight stream (server-side too) and keep whatever streamed so far. */
  function handleChatStop() {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    chatCancelRef.current[tab.id] = true;
    const requestId = chatRequestIdRef.current[tab.id];
    if (requestId) invoke("chat_cancel", { requestId }).catch(() => {});
    const partial = tab.chat.streamingText.trim();
    if (partial) {
      finalizeChat(tab.id, tab.manifest.pr_url, partial);
    } else {
      updateTab(tab.id, (t) => ({ ...t, chat: { ...t.chat, status: "idle", streamingText: "" } }));
    }
  }

  function handleChatClear() {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest) return;
    chatCancelRef.current[tab.id] = true;
    if (tab.chat.status === "streaming") {
      const requestId = chatRequestIdRef.current[tab.id];
      if (requestId) invoke("chat_cancel", { requestId }).catch(() => {});
    }
    delete chatExecutedActionsRef.current[tab.id];
    setChatActionStatuses((prev) => {
      const copy = { ...prev };
      delete copy[tab.id];
      return copy;
    });
    updateTab(tab.id, (t) => ({ ...t, chat: { ...t.chat, messages: [], status: "idle", streamingText: "", error: undefined } }));
    try {
      const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
      invoke("save_chat_history", { owner, repo, prNumber: number, state: { messages: [] } }).catch(() => {});
    } catch {
      // Non-critical.
    }
  }

  function setChatOpen(open: boolean) {
    updateTab(activeTabId, (t) => ({ ...t, chat: { ...t.chat, open } }));
  }

  // Chat and Comments are mutually exclusive right-dock panels — opening chat closes comments.
  function withChatOpen(t: Tab, open: boolean): Tab {
    return { ...t, chat: { ...t.chat, open }, commentsOpen: open ? false : t.commentsOpen };
  }

  function toggleChatOpen() {
    updateTab(activeTabId, (t) => withChatOpen(t, !t.chat.open));
  }

  function setCommentsOpen(open: boolean) {
    updateTab(activeTabId, (t) => ({ ...t, commentsOpen: open }));
  }

  function handleChatToggleWholePr(value: boolean) {
    updateTab(activeTabId, (t) => ({ ...t, chat: { ...t.chat, includeWholePr: value } }));
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
        updateTab(tabId, (t) =>
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
      // Already viewing the file — the mounted DiffViewer can jump directly.
      if (diffViewerRef.current?.revealLine(line) === false) notifyLineOutsideDiff(line);
      return;
    }
    pendingRevealLineRef.current = line ?? null;
    setSelectedFile(target);
  }

  function notifyLineOutsideDiff(line: number) {
    addToast("info", `Line ${line} doesn't exist in this file's current version.`);
  }

  /** Line to reveal once the DiffViewer for a newly-selected file mounts —
   * the viewer remounts per file (key={path}), so the jump must wait for it. */
  const pendingRevealLineRef = useRef<number | null>(null);
  useEffect(() => {
    const line = pendingRevealLineRef.current;
    if (line == null) return;
    pendingRevealLineRef.current = null;
    // Next frame, so the fresh DiffViewer instance has attached its ref.
    requestAnimationFrame(() => {
      if (diffViewerRef.current?.revealLine(line) === false) notifyLineOutsideDiff(line);
    });
  }, [selectedFilePath]);

  // ---- Chat ```marrow-action view-control blocks (issue #166) ----

  /** Run one chat-emitted view-control action against the active tab. No
   * mutating actions here — everything is navigation/view state. Returns
   * whether the action resolved (e.g. the file/commit it names actually
   * exists) so the caller can render a done/failed chip. */
  function executeChatAction(a: ChatAction): boolean {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab?.manifest) return false;
    switch (a.action) {
      case "open_file": {
        if (!resolveManifestFile(tab.manifest.files, a.path)) return false;
        handleChatOpenFile(a.path, a.line);
        return true;
      }
      case "open_overview":
        // selectedFile is left as-is (Files lens returns to where it was).
        setLens(tab.id, "overview");
        return true;
      case "next_file":
        return selectAdjacentFile(1);
      case "prev_file":
        return selectAdjacentFile(-1);
      case "open_commit": {
        const commits = tab.manifest.commits;
        const exact = commits.find((c) => c.sha === a.sha);
        // A prefix must be unambiguous — resolving to "whichever came first"
        // could open the wrong commit while reporting success.
        const prefixed = commits.filter((c) => c.sha.startsWith(a.sha));
        const match = exact ?? (prefixed.length === 1 ? prefixed[0] : undefined);
        if (!match) return false;
        handleViewCommit(match, tab.id);
        return true;
      }
      case "set_hunk_filter":
        setHunkFilter(a.filter);
        return true;
      case "set_view_mode":
        setViewMode(a.mode);
        return true;
      case "show_comments":
        // Chat and Comments are mutually exclusive right-dock panels (see
        // toggleThreadsView) — opening comments from here closes chat too.
        updateTab(tab.id, (t) => ({
          ...t,
          commentsOpen: a.open,
          chat: a.open ? { ...t.chat, open: false } : t.chat,
        }));
        return true;
      default:
        return false;
    }
  }

  /** Execute a chat action chip's click (or the streaming auto-exec effect
   * below), and record its done/failed status under the message it belongs
   * to. `msgKey` is "msg-<index>" for a finalized message or "streaming" for
   * the in-progress turn; `blockIndex` is the action's position among that
   * message's closed marrow-action fences (see RichText's parseActionFences). */
  function runChatAction(tabId: string, msgKey: string, a: ChatAction, blockIndex: number) {
    const ok = executeChatAction(a);
    const key = `${blockIndex}:${JSON.stringify(a)}`;
    setChatActionStatuses((prev) => ({
      ...prev,
      [tabId]: {
        ...prev[tabId],
        [msgKey]: { ...prev[tabId]?.[msgKey], [key]: ok ? "done" : "failed" },
      },
    }));
  }

  /** Auto-execute newly-completed ```marrow-action fences as they stream in.
   * Runs each action-block key at most once per streaming turn (tracked in
   * chatExecutedActionsRef, cleared when a new turn starts — see
   * handleChatSend) and records its outcome under the "streaming" bucket,
   * which finalizeChat then migrates to the finished message's own key. */
  useEffect(() => {
    // Deliberately active-tab-only: actions drive the CURRENT view, so a
    // stream finishing in a background tab must not yank the user around.
    // Switching back mid-stream catches up (this effect re-fires); a turn that
    // completed while backgrounded leaves its chips neutral-and-clickable,
    // same as restored history.
    if (!activeTab || activeTab.chat.status !== "streaming" || !activeTab.chat.streamingText) return;
    const tabId = activeTab.id;
    // Split on [[thought:N]] dividers before scanning, matching exactly how
    // ChatMarkdown renders this same text (see parseChatActionFences) — so a
    // divider landing inside a fence can't make the two sides disagree about
    // block indices.
    const fences = parseChatActionFences(activeTab.chat.streamingText);
    const executed = chatExecutedActionsRef.current[tabId] ?? (chatExecutedActionsRef.current[tabId] = new Set());
    fences.forEach((entry, blockIndex) => {
      if (!entry.action) return;
      // The prompt says "at most a few actions per reply", but the prompt
      // isn't enforcement: cap auto-execution per turn so a runaway reply
      // can't thrash the view. Blocks past the cap render as neutral
      // click-to-run chips instead.
      if (executed.size >= MAX_AUTO_ACTIONS_PER_TURN) return;
      const key = `${blockIndex}:${JSON.stringify(entry.action)}`;
      if (executed.has(key)) return;
      executed.add(key);
      runChatAction(tabId, "streaming", entry.action, blockIndex);
    });
  }, [activeTab?.id, activeTab?.chat.status, activeTab?.chat.streamingText]); // eslint-disable-line react-hooks/exhaustive-deps

  // Set by briefMe below; consumed once the chat-open/whole-PR state it just
  // requested has actually committed (handleChatSend reads tabsRef, which
  // only reflects this render's tabs after that commit — see the effect).
  const briefMePendingRef = useRef(false);
  /** Bumped on every briefMe() call so the send effect runs even when chat was
   * already open in whole-PR scope (no dependency would otherwise change). */
  const [briefMePing, setBriefMePing] = useState(0);

  /** AI walkthrough of the whole PR, most-important-first — opens/expands
   * chat to whole-PR scope and asks it to narrate the changes. */
  function briefMe() {
    if (!activeTab?.manifest) return;
    updateTab(activeTabId, (t) => withChatOpen(t, true));
    handleChatToggleWholePr(true);
    if (activeTab.chat.status === "streaming") return;
    briefMePendingRef.current = true;
    setBriefMePing((p) => p + 1);
  }

  useEffect(() => {
    if (!briefMePendingRef.current) return;
    if (!activeTab?.manifest) return;
    briefMePendingRef.current = false;
    handleChatSend(BRIEF_ME_PROMPT);
  }, [briefMePing]); // eslint-disable-line react-hooks/exhaustive-deps

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
        // handleSelectTab (not bare setActiveTabId) also clears the tab's
        // unread badge, matching the deep-link open path.
        handleSelectTab(alreadyOpen.id);
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
      lastPrRef: prRef,
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
  // Set by the `deep-link-resume` listener when the target tab isn't active
  // yet; consumed by the effect that advances to the next unviewed file once
  // it is (see both near the deep-link listeners above).
  const pendingResumeTabIdRef = useRef<string | null>(null);
  // Last `review-session` payload emitted (JSON), so the broadcast effect
  // below doesn't re-emit an unchanged payload on every unrelated render.
  const lastReviewSessionRef = useRef<string>("");
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
      updateTab(tabId, (t) => ({ ...t, lens, selectedFile: first ?? t.selectedFile }));
      return;
    }
    if (lens === "commits" && !tab.selectedCommit) {
      const commits = tab.manifest.commits;
      const first = commits[commits.length - 1];
      if (first) { handleViewCommit(first, tabId); return; }
    }
    updateTab(tabId, (t) => ({ ...t, lens }));
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
      if (newHighlightKeys.size > 0) {
        addToast("info", `${newHighlightKeys.size} new AI ${newHighlightKeys.size === 1 ? "note" : "notes"} from re-analysis`);
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

  // The single low-level "select a file" primitive — every open-file path
  // (sidebar clicks, search, chat citations, top-risk rows, next/prev,
  // guided review) routes through this, so always landing in the Files lens
  // needs no per-callsite changes (issue #170).
  function setSelectedFile(file: FileDiff) {
    updateTab(activeTabId, (t) => ({ ...t, selectedFile: file, lens: "files" }));
  }

  /** A change-group row on the Overview — scopes the Files sidebar to the
   * group and opens it at the group's first file (issue #170). */
  function openGroup(group: ChangeGroup, files: FileDiff[]) {
    if (!activeTabId || files.length === 0) return;
    updateTab(activeTabId, (t) => ({ ...t, groupFilter: group.label, lens: "files", selectedFile: files[0] }));
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
  }

  async function handleRequestComments() {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || !tab.manifest || tab.commentThreads.status === "loading" || tab.commentThreads.status === "loaded") return;

    updateTab(activeTabId,(t) => ({ ...t, commentThreads: { status: "loading" } }));
    try {
      const threads = await invoke<ReviewThread[]>("fetch_review_comments", {
        prUrl: tab.manifest.pr_url,
      });
      updateTab(activeTabId,(t) => ({ ...t, commentThreads: { status: "loaded", threads } }));
    } catch (err) {
      updateTab(activeTabId,(t) => ({
        ...t,
        commentThreads: { status: "error", message: String(err) },
      }));
    }
  }

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
    updateTab(tabId, (t) => ({ ...t, selectedCommit: commit, lens: "commits" }));
    loadCommitDiff(tabId, commit);
  }

  /** File-group header click in the comments panel — jump to the file, no thread scroll. */
  function handleOpenCommentFile(path: string) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab?.manifest) return;
    const file = tab.manifest.files.find((f) => f.path === path);
    if (file) setSelectedFile(file);
    else addToast("info", "File not in this diff");
  }

  /** Thread-card location click in the comments panel — select the file (if
   * needed) then scroll/flash the thread into view once the diff has mounted it. */
  function handleJumpToThread(thread: ReviewThread) {
    const tab = tabsRef.current.find((t) => t.id === activeTabId);
    if (!tab?.manifest) return;
    if (tab.selectedFile?.path !== thread.path) {
      const file = tab.manifest.files.find((f) => f.path === thread.path);
      if (!file) {
        addToast("info", "File not in this diff");
        return;
      }
      setSelectedFile(file);
    }
    pendingThreadIdRef.current = thread.id;
    setThreadScrollPing((p) => p + 1);
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
    } catch (err) {
      // Revert on error — and say so, or the button just looks dead (#72)
      updateTab(activeTabId,(t) => ({
        ...t,
        commentThreads: { status: "loaded", threads: prevThreads },
      }));
      addToast("error", `Couldn't ${resolve ? "resolve" : "unresolve"} the thread: ${String(err)}`);
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
          author: t.myReviewState?.author ?? "",
          draft: t.myReviewState?.draft ?? false,
          approved_by: t.myReviewState?.approved_by ?? [],
          status: statusMap[event] as MyReviewState["status"],
          is_re_requested: false,
          is_merged: t.myReviewState?.is_merged ?? false,
          mergeable: t.myReviewState?.mergeable ?? "",
          labels: t.myReviewState?.labels ?? [],
          last_reviewed_sha: t.myReviewState?.last_reviewed_sha ?? null,
          last_reviewed_at: t.myReviewState?.last_reviewed_at ?? null,
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

          // A merge moves neither head SHA nor comment count, so check_pr_updates
          // now reports it directly — flip the "Merged" badge without a separate
          // per-tab get_my_review_state poll (which also raced the optimistic
          // post-submit review state). Approval state stays fresh via the
          // load/refresh/submit fetches.
          if (status.merged) {
            updateTab(tab.id, (t) =>
              t.myReviewState?.is_merged
                ? t
                : {
                    ...t,
                    myReviewState: {
                      author: t.myReviewState?.author ?? "",
                      draft: t.myReviewState?.draft ?? false,
                      approved_by: t.myReviewState?.approved_by ?? [],
                      status: t.myReviewState?.status ?? "pending",
                      is_re_requested: t.myReviewState?.is_re_requested ?? false,
                      is_merged: true,
                      mergeable: t.myReviewState?.mergeable ?? "",
                      labels: t.myReviewState?.labels ?? [],
                      last_reviewed_sha: t.myReviewState?.last_reviewed_sha ?? null,
                      last_reviewed_at: t.myReviewState?.last_reviewed_at ?? null,
                    },
                  }
            );
          }

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

  // Fetch inline CI failure annotations for the active tab once its checks are
  // loaded with at least one failing run — GraphQL conclusions arrive UPPERCASE
  // (see the CiChip comment in PrOverview). Guarded by tab id + head_sha so a
  // stale resolve (tab closed, or refreshed onto a new head in the meantime)
  // never lands on the wrong state.
  useEffect(() => {
    if (!activeTab?.manifest) return;
    if (activeTab.checkAnnotations.status !== "idle") return;
    if (!activeChecks) return;
    const hasFailure = activeChecks.check_runs.some((c) => c.conclusion === "FAILURE");
    if (!hasFailure) return;

    const tabId = activeTab.id;
    const prRef = activeTab.manifest.pr_url;
    const headSha = activeTab.manifest.head_sha;

    updateTab(tabId, (t) => (t.checkAnnotations.status === "idle" ? { ...t, checkAnnotations: { status: "loading" } } : t));

    invoke<CheckFailures>("get_check_annotations", { prRef, headSha })
      .then((failures) => {
        updateTab(tabId, (t) => (t.manifest && t.manifest.head_sha === headSha ? { ...t, checkAnnotations: { status: "loaded", failures } } : t));
      })
      .catch((err) => {
        updateTab(tabId, (t) => (t.manifest && t.manifest.head_sha === headSha ? { ...t, checkAnnotations: { status: "error", message: String(err) } } : t));
      });
  }, [activeTab?.id, activeTab?.manifest?.pr_url, activeTab?.manifest?.head_sha, activeTab?.checkAnnotations.status, activeChecks]); // eslint-disable-line react-hooks/exhaustive-deps

  // Floating mini-player visibility:
  //  - HIDE it whenever the MAIN window gains focus (you've engaged the app).
  //    Interacting with the floating panel focuses the panel, not the main
  //    window, so dragging/resizing/clicking it never hides it — only genuinely
  //    going to the main window does.
  //  - SHOW it when you leave Marrow. On macOS that's driven by the native
  //    app-active poll in the Rust backend (webview blur is unreliable for
  //    app-switches there); on other platforms, on the main window's blur.
  useEffect(() => {
    const setVisible = (visible: boolean) =>
      invoke("set_activity_window_visible", { visible }).catch(() => {});
    const hide = () => setVisible(false);
    const unlisten = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused) hide();
    });
    window.addEventListener("focus", hide);

    const isMac = navigator.userAgent.includes("Macintosh");
    const show = () => setVisible(true);
    if (!isMac) window.addEventListener("blur", show);

    return () => {
      unlisten.then((fn) => fn());
      window.removeEventListener("focus", hide);
      if (!isMac) window.removeEventListener("blur", show);
    };
  }, []);

  // Re-load dismissed-highlight state from disk when the window regains focus, so
  // dismissals made outside the app (e.g. an external/AI tool writing the
  // ~/.config/marrow/dismissed file) show up without reopening the PR.
  useEffect(() => {
    const unlisten = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused) return;
      for (const tab of tabsRef.current) {
        if (!tab.manifest) continue;
        const { owner, repo, number } = parsePrUrl(tab.manifest.pr_url);
        invoke<{ keys: string[]; resolutions?: Record<string, NoteResolution> } | null>("load_dismissed_highlights", { owner, repo, prNumber: number })
          .then((saved) => {
            const keys = saved?.keys ?? [];
            const resolutions = saved?.resolutions ?? {};
            updateTab(tab.id, (t) => {
              const sameKeys = t.dismissedHighlights.size === keys.length && keys.every((k) => t.dismissedHighlights.has(k));
              // Resolutions must be compared too, or a metadata-only change
              // (e.g. the resolve script adding a reason to an existing key)
              // would be invisible until restart.
              const entries = Object.entries(resolutions);
              const sameRes = t.noteResolutions.size === entries.length && entries.every(([k, r]) => {
                const cur = t.noteResolutions.get(k);
                return !!cur && cur.state === r.state && (cur.reason ?? "") === (r.reason ?? "") && (cur.at ?? "") === (r.at ?? "");
              });
              if (sameKeys && sameRes) return t;
              return { ...t, dismissedHighlights: new Set(keys), noteResolutions: new Map(entries) };
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
            lens: t.lens,
            // Retired with the comments panel (#144) — kept on the wire type
            // for schema stability, always written null now.
            selected_comment_file: null,
          })),
        active_pr: tabs.find((t) => t.id === activeTabId)?.manifest?.pr_url ?? null,
      };
      invoke("save_session", { state }).catch(() => {});
    }, 500);
    return () => clearTimeout(timer);
  }, [tabs, activeTabId]);

  // Broadcast the active tab's review session to every window (the mini-player
  // widget's "Now Reviewing" card). Null when the active tab has no manifest
  // (queue home) — the widget hides the card. Cheap event, so no debounce, but
  // skip re-emitting an unchanged payload (e.g. re-renders that don't actually
  // move viewedCount/nextFile).
  useEffect(() => {
    const manifest = activeTab?.manifest ?? null;
    // Typed as ReviewSession so this inline emit can't drift from the shape
    // the widget's useReviewSession listener expects.
    const payload: ReviewSession | null = manifest
      ? {
          prUrl: manifest.pr_url,
          prRef: (() => {
            const { owner, repo, number } = parsePrUrl(manifest.pr_url);
            return `${owner}/${repo}#${number}`;
          })(),
          number: manifest.pr_number,
          title: manifest.pr_title,
          viewedCount: activeTab?.viewedFiles.size ?? 0,
          relevantCount: manifest.files.filter((f) => f.classification !== "NOT_RELEVANT").length,
          nextFile: nextUnviewed(guidedOrder(), -1)?.path ?? null,
        }
      : null;
    const serialized = JSON.stringify(payload);
    if (serialized === lastReviewSessionRef.current) return;
    lastReviewSessionRef.current = serialized;
    emit("review-session", payload).catch(() => {});
  }, [activeTabId, activeTab?.manifest, activeTab?.viewedFiles]); // eslint-disable-line react-hooks/exhaustive-deps

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

  // Command palette registry — searchable home for every action, with the
  // keyboard hint teaching the direct shortcut. Review commands only appear
  // when a PR is loaded.
  const paletteCommands: PaletteCommand[] = [];
  if (activeTab?.manifest) {
    const m = activeTab.manifest;
    paletteCommands.push(
      { id: "overview", section: "Review", title: "Back to overview", run: () => { if (activeTabId) setLens(activeTabId, "overview"); } },
      { id: "next-file", section: "Review", title: "Next file", keys: "]", run: () => selectAdjacentFile(1) },
      { id: "prev-file", section: "Review", title: "Previous file", keys: "[", run: () => selectAdjacentFile(-1) },
      { id: "mark-viewed", section: "Review", title: "Mark file reviewed", keys: "V", run: () => { const p = activeTab.selectedFile?.path; if (p) toggleViewed(p); } },
      { id: "mark-next", section: "Review", title: "Mark reviewed and go to next", run: markReviewedAndAdvance },
      { id: "finish", section: "Review", title: "Finish review…", keys: "R", run: () => setReviewPickerOpen(true) },
      { id: "search", section: "Review", title: "Search in diffs", keys: "/", run: () => searchRef.current?.open("local") },
      { id: "threads", section: "Review", title: "Toggle comments panel", keys: "T", run: toggleThreadsView },
      { id: "refresh", section: "Review", title: "Refresh PR", keys: "⌃R", run: handleRefreshPr },
      { id: "github", section: "Review", title: "Open PR on GitHub", run: () => { openUrl(m.pr_url); } },
      { id: "ask-ai", section: "Review", title: "Ask AI about this change", keys: "⌘J", run: toggleChatOpen },
      { id: "brief-me", section: "Review", title: "Brief me — AI walkthrough of this PR", run: briefMe },
      { id: "view-split", section: "View", title: "Split diff view", run: () => setViewMode("split") },
      { id: "view-unified", section: "View", title: "Unified diff view", run: () => setViewMode("unified") },
      { id: "toggle-sig", section: "View", title: showHunkSignificance ? "Hide hunk significance" : "Show hunk significance", run: () => setShowHunkSignificance((v) => !v) },
      { id: "toggle-notes", section: "View", title: showAiNotes ? "Hide AI notes" : "Show AI notes", run: () => setShowAiNotes((v) => !v) },
    );
  }
  paletteCommands.push(
    { id: "new-tab", section: "App", title: "New review tab", keys: "⌃T", run: handleNewReview },
    { id: "settings", section: "App", title: "Settings…", run: () => setSettingsOpen(true) },
    { id: "updates", section: "App", title: "Check for updates", run: () => checkForUpdates(false) },
    { id: "help", section: "App", title: "Keyboard shortcuts", keys: "?", run: () => setHelpOpen(true) },
  );

  const overlays = (
    <>
      {welcomeOpen && (
        <WelcomeSetup
          onDone={() => setWelcomeOpen(false)}
          onOpenSettings={() => setSettingsOpen(true)}
        />
      )}
      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        commands={paletteCommands}
      />
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
    <div className={`app${activeTab?.chat.open || activeTab?.commentsOpen ? " app--right-panel" : ""}`}>
      <ActivityWidget onOpenPr={(ref) => handleFetchStart(ref, activeTabId ?? undefined)} />
      <Header
        tabs={tabs}
        activeTabId={activeTabId}
        onSelectTab={handleSelectTab}
        onCloseTab={closeTab}
        onNewReview={handleNewReview}
        viewedCount={activeTab?.viewedFiles.size ?? 0}
        lens={activeTab?.lens ?? "overview"}
        onSetLens={(lens) => { if (activeTabId) setLens(activeTabId, lens); }}
        filesCount={filesLensCount}
        commitsCount={commitsLensCount}
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
        onOpenPalette={() => setPaletteOpen(true)}
        chatOpen={activeTab?.chat.open ?? false}
        onToggleChat={activeTab?.manifest ? toggleChatOpen : undefined}
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
          {activeTab.loading ? (
            <div className="empty-message">
              <LoadingView
                prRef={activeTab.loading.prRef}
                prTitle={activeTab.loading.prTitle}
                progress={activeTab.loading.progress}
                fileCounts={activeTab.loading.fileCounts}
                onCancel={() => handleFetchCancel(activeTab.id)}
              />
            </div>
          ) : (
            <div className="queue-home">
              <PrOpener
                onFetchStart={(ref) => handleFetchStart(ref, activeTab.id)}
                onFilterChange={setQueueFilter}
                onSettingsClick={() => setSettingsOpen(true)}
                onCheckForUpdates={() => checkForUpdates(false)}
                viewerLogin={viewerLogin}
              />
              {activeTab.error && (
                <div className="opener-error" role="alert">
                  <strong>
                    Couldn't load {activeTab.lastPrRef ?? "the PR"}
                  </strong>
                  <pre>{activeTab.error}</pre>
                  {activeTab.lastPrRef && (
                    <button
                      className="opener-retry"
                      onClick={() => handleFetchStart(activeTab.lastPrRef!, activeTab.id)}
                    >
                      Try again
                    </button>
                  )}
                </div>
              )}
              <ReviewRequestList
                onSelectPr={(ref) => handleFetchStart(ref, activeTab.id)}
                onSelectCachedPr={handleOpenCachedPr}
                openPrUrls={openPrUrls}
                filter={queueFilter}
                onOpenSettings={() => setSettingsOpen(true)}
              />
              {staleConfirm && (
                <div className="settings-overlay" onMouseDown={() => setStaleConfirm(null)}>
                  <div className="welcome-card" onMouseDown={(e) => e.stopPropagation()}>
                    <h3>This PR has new commits</h3>
                    <p>
                      "{staleConfirm.title}" changed since it was analyzed.
                      Opening it now will run the AI analysis again on the
                      latest version.
                    </p>
                    <div className="welcome-actions">
                      <button
                        className="welcome-primary"
                        onClick={() => { const ref = staleConfirm.prRef; setStaleConfirm(null); handleFetchStart(ref, activeTab.id); }}
                      >
                        Analyze updated PR
                      </button>
                      <button className="welcome-skip" onClick={() => setStaleConfirm(null)}>
                        Cancel
                      </button>
                    </div>
                  </div>
                </div>
              )}
              <div className="queue-drop-hint">
                Tip: drop a manifest JSON file anywhere here to load a review.
              </div>
            </div>
          )}
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
          {activeTab.lens === "commits" ? (
            <CommitsLens
              commits={activeTab.manifest.commits}
              selectedCommit={activeTab.selectedCommit}
              diff={commitDiff}
              loading={commitDiffLoading}
              error={commitDiffError}
              commitDiffCache={commitDiffCacheRef.current}
              repoBaseUrl={repoBaseUrl(activeTab.manifest.pr_url)}
              onSelectCommit={(c) => handleViewCommit(c)}
              onViewCumulativeDiff={() => { if (activeTabId) setLens(activeTabId, "files"); }}
            />
          ) : activeTab.lens === "overview" ? (
            <PrOverview
              manifest={activeTab.manifest}
              checksStatus={activeChecks ?? null}
              checkFailures={activeCheckFailures}
              reviewState={activeTab.myReviewState ?? null}
              viewedCount={activeTab.viewedFiles.size}
              unresolvedThreads={activeTab.commentThreads.status === "loaded" ? activeTab.commentThreads.threads.filter((t) => !t.is_resolved).length : null}
              hasSubmittedReview={activeTab.myReviewState != null && activeTab.myReviewState.status !== "pending" && activeTab.myReviewState.status !== "dismissed" && !activeTab.myReviewState.is_re_requested}
              startTarget={nextUnviewed(guidedOrder(), -1)}
              onStartReview={() => { const t = nextUnviewed(guidedOrder(), -1); if (t) setSelectedFile(t); }}
              onSelectFile={setSelectedFile}
              onOpenGroup={openGroup}
              onOpenAt={handleChatOpenFile}
              onBriefMe={briefMe}
              onViewCommit={handleViewCommit}
              newHighlightKeys={
                // Dismissing a new note removes it from the chip immediately —
                // a dead "1 new AI note" pointing at a hidden note is worse
                // than no chip.
                activeTab.newHighlightKeys &&
                new Set([...activeTab.newHighlightKeys].filter((k) => !activeTab.dismissedHighlights.has(k)))
              }
            />
          ) : (
          <>
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
            checkFailureCounts={checkFailureCounts}
            commentsOpen={activeTab.commentsOpen ?? false}
            onToggleComments={toggleThreadsView}
            onVisibleFilesChange={handleVisibleFilesChange}
            groupFilter={activeTab.groupFilter}
            onClearGroupFilter={() => { if (activeTabId) updateTab(activeTabId, (t) => ({ ...t, groupFilter: null })); }}
          />
          <div className="diff-pane">
            {activeTab.selectedFile ? (
              (() => {
                const order = guidedOrder();
                const idx = order.indexOf(activeTab.selectedFile.path);
                const allReviewed = order.length > 0 && order.every((p) => activeTab.viewedFiles.has(p));
                // Exclude the open file so "Next" never points at itself when
                // it's the last unviewed one.
                const next = nextUnviewed(order, idx, activeTab.selectedFile.path);
                return (
                  <>
                    <DiffViewer ref={diffViewerRef} key={activeTab.selectedFile.path} file={activeTab.selectedFile} viewMode={viewMode} onViewModeChange={setViewMode} showHunkSignificance={showHunkSignificance} showAiNotes={showAiNotes} expandAllHunks={expandAllHunks} dismissedHighlights={activeTab.dismissedHighlights} noteResolutions={activeTab.noteResolutions} newHighlightKeys={activeTab.newHighlightKeys} onResolveHighlight={resolveHighlight} onRestoreHighlight={restoreHighlight} onCreateComment={handleCreateComment} onEditComment={handleEditComment} onReply={handleReply} onToggleResolved={handleToggleResolved} onToggleReaction={handleToggleReaction} reviewThreads={activeTab.commentThreads.status === "loaded" ? activeTab.commentThreads.threads : undefined} checkAnnotations={selectedFileAnnotations} searchMatches={fileSearchMatches} currentSearchMatch={currentSearchMatch} searchQuery={searchQuery} />
                    <NextFileBar
                      index={idx >= 0 ? idx : 0}
                      total={order.length}
                      isViewed={activeTab.viewedFiles.has(activeTab.selectedFile.path)}
                      nextName={next ? next.path.split("/").pop() ?? next.path : null}
                      nextRationale={next ? triageRationale(next.path) : null}
                      allReviewed={allReviewed}
                      onMarkReviewed={markReviewedAndAdvance}
                      onNext={() => { if (next) setSelectedFile(next); }}
                      onComment={() => diffViewerRef.current?.commentAtCursor()}
                      onFinishReview={() => setReviewPickerOpen(true)}
                    />
                  </>
                );
              })()
            ) : (
              <div className="no-file-selected">Select a file to review</div>
            )}
          </div>
          </>
          )}
          {activeTab.chat.open && (
            <ChatPanel
              chat={activeTab.chat}
              selectedFilePath={activeTab.selectedFile?.path ?? null}
              filePaths={activeTab.manifest.files.map((f) => f.path)}
              onSend={handleChatSend}
              onStop={handleChatStop}
              onClose={() => setChatOpen(false)}
              onClear={handleChatClear}
              onToggleWholePr={handleChatToggleWholePr}
              onOpenFile={handleChatOpenFile}
              onRunAction={(msgKey, a, blockIndex) => runChatAction(activeTab.id, msgKey, a, blockIndex)}
              actionStatuses={chatActionStatuses[activeTab.id]}
            />
          )}
          {activeTab.commentsOpen && (
            <CommentsPanel
              commentThreads={activeTab.commentThreads}
              onRetry={handleRequestComments}
              onReply={handleReply}
              onToggleResolved={handleToggleResolved}
              onEditComment={handleEditComment}
              onToggleReaction={handleToggleReaction}
              onClose={() => setCommentsOpen(false)}
              onOpenFile={handleOpenCommentFile}
              onJumpToThread={handleJumpToThread}
            />
          )}
        </div>
        </div>
      )}
      {overlays}
    </div>
  );
}

export default App;
