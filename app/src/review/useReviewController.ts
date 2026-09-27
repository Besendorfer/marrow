// Builds the per-render review context and runs every App-level effect in
// its original order (issue #238 phase 2). App.tsx renders from the
// returned ctx; new surfaces (the inbox) can call its handlers directly.

import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";
import { parseChatActionFences } from "../components/RichText";
import { open as openUrl } from "@tauri-apps/plugin-shell";
import { useKeyboardShortcuts } from "../hooks/useKeyboardShortcuts";
import { exit } from "@tauri-apps/plugin-process";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { ReviewManifest, Tab, SidebarView, PrUpdateStatus, PrChecksStatus, SessionState, Settings, NoteResolution, CheckFailures } from "../types";
import { parsePrUrl, extractPrRef, canonicalPrKey, isFailingCheck } from "../utils";
import type { ReviewSession } from "../hooks/useActivityFeed";
import { isOpenerTab, BRIEF_ME_PROMPT, MAX_AUTO_ACTIONS_PER_TURN } from "./helpers";
import { useReviewState } from "./useReviewState";
import { createTabs } from "./tabs";
import { createNavigation } from "./navigation";
import { createLoading } from "./loading";
import { createProgress } from "./progress";
import { createChecks } from "./checks";
import { createChat } from "./chat";
import { createComments } from "./comments";
import { createCommits } from "./commits";
import type { ReviewCtx } from "./ctx";

export function useReviewController(): ReviewCtx {
  const ctx = { ...useReviewState() } as ReviewCtx;
  // Every factory runs before any handler lands on ctx, so a factory may
  // destructure STATE from ctx at creation time but must reach another
  // module's handler as `ctx.fn(...)` inside a function body — destructuring
  // a handler up front would capture undefined.
  Object.assign(ctx,
    createTabs(ctx),
    createNavigation(ctx),
    createLoading(ctx),
    createProgress(ctx),
    createChecks(ctx),
    createChat(ctx),
    createComments(ctx),
    createCommits(ctx),
  );
  const {
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
    setExpandAllHunks,
    settingsOpen,
    helpOpen,
    setHelpOpen,
    setCommitDiffLoading,
    setCommitDiffError,
    setCommitDiff,
    paletteOpen,
    setPaletteOpen,
    welcomeOpen,
    setWelcomeOpen,
    reviewPickerOpen,
    setReviewPickerOpen,
    searchOpen,
    setViewerLogin,
    searchRef,
    diffViewerRef,
    pendingThreadIdRef,
    threadScrollPing,
    setChecksMap,
    chatExecutedActionsRef,
    sessionRestoredRef,
    settingsRef,
    fetchingRef,
    refreshFingerprint,
    addToast,
    checkForUpdates,
    activeTab,
    activeChecks,
    showChecksModal,
    selectedFilePath,
    resumePing,
    setResumePing,
    pendingRevealLineRef,
    pendingComposerRef,
    briefMePendingRef,
    briefMePing,
    tabsRef,
    activeTabIdRef,
    pendingResumeTabIdRef,
    lastReviewSessionRef,
    closeTabRef,
    newTabRef,
    setShowQuitHint,
    quitArmedRef,
    quitTimerRef,
    checksMapRef,
    checksDismissedRef,
    guidedOrder,
    nextUnviewed,
    selectAdjacentFile,
    selectAdjacentTab,
    toggleThreadsView,
    createTab,
    handleNewReview,
    handleSelectTab,
    loadManifest,
    fetchMyReviewState,
    fetchChecksStatus,
    loadPersistedViewedState,
    loadDismissedHighlights,
    loadResolvedSpecs,
    loadLocalRequirements,
    loadChatHistory,
    handleChatSend,
    toggleChatOpen,
    notifyLineOutsideDiff,
    runChatAction,
    handleFetchStart,
    updateTab,
    setLens,
    handleRefreshPr,
    handleRefreshComments,
    setSelectedFile,
    toggleViewed,
    handleRequestComments,
    loadCommitDiff,
    closeTab,
  } = ctx;

  useEffect(() => { refreshFingerprint(); }, [refreshFingerprint]);

  // The review picker submits against the active tab, so it must not outlive
  // the tab it was opened on: switching or closing tabs dismisses it
  // (issue #238 — it used to float over the queue after the PR tab closed).
  useEffect(() => { setReviewPickerOpen(false); }, [activeTabId]);

  useEffect(() => {
    if (import.meta.env.DEV) return;
    const startupTimer = setTimeout(() => checkForUpdates(true), 5000);
    const interval = setInterval(() => checkForUpdates(true), 6 * 60 * 60 * 1000);
    return () => {
      clearTimeout(startupTimer);
      clearInterval(interval);
    };
  }, [checkForUpdates]);

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
                if (entry.lens === "files" || entry.lens === "commits" || entry.lens === "checks") {
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
              loadResolvedSpecs(tab);
              loadLocalRequirements(tab);
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
  useEffect(() => {
    const composer = pendingComposerRef.current;
    if (composer) {
      pendingComposerRef.current = null;
      // Next frame, so the fresh DiffViewer instance has attached its ref.
      requestAnimationFrame(() => {
        diffViewerRef.current?.openComposer(composer.startLine, composer.endLine, composer.side, composer.initialBody);
      });
    }
    const line = pendingRevealLineRef.current;
    if (line == null) return;
    pendingRevealLineRef.current = null;
    // Next frame, so the fresh DiffViewer instance has attached its ref.
    requestAnimationFrame(() => {
      if (diffViewerRef.current?.revealLine(line) === false) notifyLineOutsideDiff(line);
    });
    // Lens is a dep so a reveal deferred by the same-file-wrong-lens path in
    // handleChatOpenFile fires when the Files lens (re)mounts the viewer.
  }, [selectedFilePath, activeTab?.lens]);

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
      // draft_comment / draft_pr_comment are manual-only: never auto-run (they
      // would pop a composer in the user's face mid-stream), and they don't
      // consume the auto-exec cap or the executed set — their chips stay
      // neutral until clicked.
      if (entry.action.action === "draft_comment" || entry.action.action === "draft_pr_comment") return;
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

  useEffect(() => {
    if (!briefMePendingRef.current) return;
    if (!activeTab?.manifest) return;
    briefMePendingRef.current = false;
    handleChatSend(BRIEF_ME_PROMPT);
  }, [briefMePing]); // eslint-disable-line react-hooks/exhaustive-deps
  newTabRef.current = handleNewReview;
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
  // (see the CiChip comment in PrOverview) — or once the user enters the
  // Checks lens directly (issue #175), where the fetch is a cheap no-op on a
  // green PR (empty annotations). Guarded by tab id + head_sha so a stale
  // resolve (tab closed, or refreshed onto a new head in the meantime) never
  // lands on the wrong state.
  useEffect(() => {
    if (!activeTab?.manifest) return;
    if (activeTab.checkAnnotations.status !== "idle") return;
    if (!activeChecks) return;
    const hasFailure = activeChecks.check_runs.some(isFailingCheck);
    if (!hasFailure && activeTab.lens !== "checks") return;

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
  }, [activeTab?.id, activeTab?.manifest?.pr_url, activeTab?.manifest?.head_sha, activeTab?.checkAnnotations.status, activeChecks, activeTab?.lens]); // eslint-disable-line react-hooks/exhaustive-deps

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

  // Re-load dismissed-highlight and resolved-spec state from disk when the
  // window regains focus, so resolutions made outside the app (e.g. an
  // external/AI tool writing the ~/.config/marrow/dismissed or resolved_specs
  // files) show up without reopening the PR.
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
        // Same drill for the requirements card's resolved specs — previously
        // only loaded at PR-open, so external writes were invisible until the
        // tab was reopened.
        invoke<{ keys: string[]; resolutions?: Record<string, NoteResolution> } | null>("load_resolved_specs", { owner, repo, prNumber: number })
          .then((saved) => {
            const keys = saved?.keys ?? [];
            const resolutions = saved?.resolutions ?? {};
            updateTab(tab.id, (t) => {
              const sameKeys = t.resolvedSpecKeys.size === keys.length && keys.every((k) => t.resolvedSpecKeys.has(k));
              const entries = Object.entries(resolutions);
              const sameRes = t.specResolutions.size === entries.length && entries.every(([k, r]) => {
                const cur = t.specResolutions.get(k);
                return !!cur && cur.state === r.state && (cur.reason ?? "") === (r.reason ?? "") && (cur.at ?? "") === (r.at ?? "");
              });
              if (sameKeys && sameRes) return t;
              return { ...t, resolvedSpecKeys: new Set(keys), specResolutions: new Map(entries) };
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

  return ctx;
}
