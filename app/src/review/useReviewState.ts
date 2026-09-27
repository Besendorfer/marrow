// All App-level state, refs, memos, and callbacks (moved verbatim from
// App.tsx in issue #238 phase 2 — declaration order preserved). No effects
// live here: they run in useReviewController, in their original order.

import { useState, useRef, useMemo, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { DiffViewerHandle } from "../components/DiffViewer";
import type { SearchBarHandle } from "../components/SearchBar";
import { createToast, type ToastData } from "../components/Toast";
import { check } from "@tauri-apps/plugin-updater";
import type { DiffViewMode, Tab, HunkSignificanceFilter, PrChecksStatus, UpdateStatus, Settings, CommitDiff, CheckAnnotation } from "../types";

export function useReviewState() {
  const nextTabId = useRef(1);
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeTabId, setActiveTabId] = useState<string | null>(null);
  const [viewMode, setViewMode] = useState<DiffViewMode>("split");
  const [showHunkSignificance, setShowHunkSignificance] = useState(true);
  const [showAiNotes, setShowAiNotes] = useState(true);
  const [hunkFilter, setHunkFilter] = useState<HunkSignificanceFilter>("all");
  const [expandAllHunks, setExpandAllHunks] = useState(false);
  const [inboxLayout, setInboxLayout] = useState(false);
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

  // Fingerprint of the CURRENT analysis environment (issue #202) — compared
  // against each manifest's stored fingerprint to flag stale caches. Loaded
  // on mount and re-loaded when settings close (a model/provider change
  // changes it). Empty until known: no manifest is flagged on a blank value.
  const [currentFingerprint, setCurrentFingerprint] = useState("");
  const refreshFingerprint = useCallback(() => {
    invoke<string>("current_analysis_fingerprint")
      .then(setCurrentFingerprint)
      .catch(() => {});
  }, []);

  const handleSettingsClose = useCallback(() => {
    setSettingsOpen(false);
    invoke<Settings>("get_settings").then((s) => {
      settingsRef.current = s;
      setExpandAllHunks(s.expand_all_hunks ?? false);
      setInboxLayout(s.inbox_layout ?? false);
    }).catch(() => {});
    refreshFingerprint();
  }, [refreshFingerprint]);

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

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? null;
  const openPrUrls = useMemo(
    () => new Set(tabs.filter((t) => t.manifest).map((t) => t.manifest!.pr_url)),
    [tabs]
  );

  const activeChecks = activeTab ? checksMap[activeTab.id] : undefined;

  // Lens switcher segment counts (issue #170). Files count mirrors the
  // relevant/fallback-to-total rule buildChatFiles uses for whole-PR chat scope.
  const relevantFileCount = activeTab?.manifest?.files.filter((f) => f.classification === "RELEVANT").length ?? 0;
  const filesLensCount = activeTab?.manifest ? (relevantFileCount > 0 ? relevantFileCount : activeTab.manifest.files.length) : 0;
  const commitsLensCount = activeTab?.manifest?.commits.length ?? 0;

  const selectedFilePath = activeTab?.selectedFile?.path ?? null;

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
  // Paths in the active PR's diff — the Checks lens uses this to tell a
  // jumpable annotation from a run-level one anchored outside the diff
  // (e.g. `.github`), same distinction the old Overview card made via byPath.
  const diffFilePaths = useMemo(
    () => new Set(activeTab?.manifest?.files.map((f) => f.path) ?? []),
    [activeTab?.manifest]
  );

  /** Bumped per resume event so the advance effect runs even without a tab-id transition. */
  const [resumePing, setResumePing] = useState(0);

  // ---- Conversational diff Q&A (chat) ----

  // Per-tab flag: when true, in-flight stream events for that tab are ignored
  // (the user pressed Stop, cleared the chat, or sent a fresh message).
  const chatCancelRef = useRef<Record<string, boolean>>({});
  // Per-tab id of the in-flight chat request, so Stop can abort it server-side.
  const chatRequestIdRef = useRef<Record<string, string>>({});

  /** Line to reveal once the DiffViewer for a newly-selected file mounts —
   * the viewer remounts per file (key={path}), so the jump must wait for it. */
  const pendingRevealLineRef = useRef<number | null>(null);
  /** Composer to open once the DiffViewer for a newly-selected file mounts —
   * set by the chat draft_comment action when the target file/lens isn't the
   * one currently rendered; same deferral as pendingRevealLineRef. */
  const pendingComposerRef = useRef<{ startLine: number; endLine: number; side: "LEFT" | "RIGHT"; initialBody: string } | null>(null);

  // Set by briefMe below; consumed once the chat-open/whole-PR state it just
  // requested has actually committed (handleChatSend reads tabsRef, which
  // only reflects this render's tabs after that commit — see the effect).
  const briefMePendingRef = useRef(false);
  /** Bumped on every briefMe() call so the send effect runs even when chat was
   * already open in whole-PR scope (no dependency would otherwise change). */
  const [briefMePing, setBriefMePing] = useState(0);

  const unlistenRef = useRef<(() => void) | null>(null);

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
  // Chrome-style confirm-quit: first Cmd+Q arms a hint, a second within the window quits.
  const [showQuitHint, setShowQuitHint] = useState(false);
  const quitArmedRef = useRef(false);
  const quitTimerRef = useRef<number | null>(null);
  const checksMapRef = useRef(checksMap);
  checksMapRef.current = checksMap;

  return {
    nextTabId,
    tabs,
    setTabs,
    activeTabId,
    setActiveTabId,
    viewMode,
    setViewMode,
    showHunkSignificance,
    setShowHunkSignificance,
    showAiNotes,
    setShowAiNotes,
    hunkFilter,
    setHunkFilter,
    expandAllHunks,
    setExpandAllHunks,
    inboxLayout,
    setInboxLayout,
    error,
    setError,
    settingsOpen,
    setSettingsOpen,
    helpOpen,
    setHelpOpen,
    commitDiffLoading,
    setCommitDiffLoading,
    commitDiffError,
    setCommitDiffError,
    commitDiff,
    setCommitDiff,
    commitDiffCacheRef,
    commitDiffFetchingRef,
    paletteOpen,
    setPaletteOpen,
    welcomeOpen,
    setWelcomeOpen,
    staleConfirm,
    setStaleConfirm,
    searchOpen,
    setSearchOpen,
    queueFilter,
    setQueueFilter,
    viewerLogin,
    setViewerLogin,
    searchRef,
    diffViewerRef,
    pendingThreadIdRef,
    threadScrollPing,
    setThreadScrollPing,
    visibleOrderRef,
    handleVisibleFilesChange,
    toasts,
    setToasts,
    checksMap,
    setChecksMap,
    chatActionStatuses,
    setChatActionStatuses,
    chatExecutedActionsRef,
    updateStatus,
    setUpdateStatus,
    updateStatusRef,
    pendingUpdateRef,
    sessionRestoredRef,
    settingsRef,
    fetchingRef,
    fetchTokenRef,
    currentFingerprint,
    setCurrentFingerprint,
    refreshFingerprint,
    handleSettingsClose,
    addToast,
    removeToast,
    checkForUpdates,
    handleDownloadUpdate,
    activeTab,
    openPrUrls,
    activeChecks,
    relevantFileCount,
    filesLensCount,
    commitsLensCount,
    selectedFilePath,
    annotationsByPath,
    checkFailureCounts,
    selectedFileAnnotations,
    diffFilePaths,
    resumePing,
    setResumePing,
    chatCancelRef,
    chatRequestIdRef,
    pendingRevealLineRef,
    pendingComposerRef,
    briefMePendingRef,
    briefMePing,
    setBriefMePing,
    unlistenRef,
    tabsRef,
    activeTabIdRef,
    pendingResumeTabIdRef,
    lastReviewSessionRef,
    closeTabRef,
    newTabRef,
    showQuitHint,
    setShowQuitHint,
    quitArmedRef,
    quitTimerRef,
    checksMapRef,
  };
}

export type ReviewState = ReturnType<typeof useReviewState>;
