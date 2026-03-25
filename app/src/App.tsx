import { useState, useEffect, useRef, useMemo, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { FileSidebar } from "./components/FileSidebar";
import { DiffViewer, detectLanguage } from "./components/DiffViewer";
import { CommentsViewer } from "./components/CommentsViewer";
import { Header } from "./components/Header";
import { PrOpener } from "./components/PrOpener";
import { ReviewRequestList } from "./components/ReviewRequestList";
import { LoadingView } from "./components/LoadingView";
import { SettingsModal } from "./components/SettingsModal";
import { SummaryParagraphs } from "./components/SummaryParagraphs";
import { SearchBar } from "./components/SearchBar";
import { ToastContainer, createToast, type ToastData } from "./components/Toast";
import type { ReviewManifest, FileDiff, DiffViewMode, Tab, FetchProgress, HunkSignificanceFilter, SidebarView, ReviewThread, ReviewComment, SearchMatch, PrUpdateStatus } from "./types";

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
  const [showOpener, setShowOpener] = useState(false);
  const [loading, setLoading] = useState(false);
  const [loadingPrRef, setLoadingPrRef] = useState("");
  const [loadingPrTitle, setLoadingPrTitle] = useState<string | null>(null);
  const [progress, setProgress] = useState<FetchProgress | null>(null);
  const [fileCounts, setFileCounts] = useState<Record<number, { done: number; total: number }>>({});
  const [searchMatches, setSearchMatches] = useState<SearchMatch[]>([]);
  const [searchCurrentIndex, setSearchCurrentIndex] = useState(0);
  const [searchQuery, setSearchQuery] = useState("");
  const [toasts, setToasts] = useState<ToastData[]>([]);

  const addToast = useCallback((type: ToastData["type"], message: string) => {
    setToasts((prev) => [...prev, createToast(type, message)]);
  }, []);

  const removeToast = useCallback((id: string) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const activeTab = tabs.find((t) => t.id === activeTabId) ?? null;

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

  function createTab(manifest: ReviewManifest): Tab {
    const hasGroups = (manifest.change_groups ?? []).length > 0;
    return {
      id: String(nextTabId.current++),
      manifest,
      selectedFile: manifest.files.length > 0 ? manifest.files[0] : null,
      viewedFiles: new Set(),
      commentThreads: { status: "idle" },
      selectedCommentFile: null,
      sidebarView: hasGroups ? "groups" : "category",
      isRefreshing: false,
      lastCommentCount: 0,
    };
  }

  useEffect(() => {
    invoke<string | null>("get_initial_manifest_path").then((path) => {
      if (path) {
        loadManifest(path);
      }
    });
  }, []);

  async function loadManifest(path: string) {
    try {
      const data = await invoke<ReviewManifest>("load_manifest", { path });
      handleManifestLoaded(data);
    } catch (e) {
      setError(String(e));
    }
  }

  function handleManifestLoaded(data: ReviewManifest) {
    const tab = createTab(data);
    setTabs((prev) => [...prev, tab]);
    setActiveTabId(tab.id);
    setShowOpener(false);
    setError(null);
  }

  const unlistenRef = useRef<(() => void) | null>(null);

  async function handleFetchStart(prRef: string) {
    if (loading) return;
    setLoading(true);
    setLoadingPrRef(prRef);
    setLoadingPrTitle(null);
    setProgress(null);
    setFileCounts({});
    setError(null);

    const unlisten = await listen<FetchProgress>("fetch-progress", (event) => {
      setProgress(event.payload);
      if (event.payload.pr_title) {
        setLoadingPrTitle(event.payload.pr_title);
      }
      if (event.payload.files_total != null) {
        setFileCounts((prev) => ({
          ...prev,
          [event.payload.step]: {
            done: event.payload.files_done ?? 0,
            total: event.payload.files_total!,
          },
        }));
      }
    });
    unlistenRef.current = unlisten;

    try {
      const manifest = await invoke<ReviewManifest>("fetch_pr", { prRef });
      handleManifestLoaded(manifest);
    } catch (err) {
      setError(String(err));
    } finally {
      unlisten();
      unlistenRef.current = null;
      setLoading(false);
      setProgress(null);
    }
  }

  function handleFetchCancel() {
    unlistenRef.current?.();
    unlistenRef.current = null;
    setLoading(false);
    setProgress(null);
  }

  const tabsRef = useRef(tabs);
  tabsRef.current = tabs;
  const loadingRef = useRef(loading);
  loadingRef.current = loading;

  function updateTab(tabId: string | null, updater: (tab: Tab) => Tab) {
    setTabs((prev) => prev.map((t) => (t.id === tabId ? updater(t) : t)));
  }

  async function handleRefreshPr(tabId?: string) {
    const targetId = tabId ?? activeTabId;
    const tab = tabsRef.current.find((t) => t.id === targetId);
    if (!tab || tab.isRefreshing || loadingRef.current) return;

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

      const preservedViewed = new Set(
        [...tab.viewedFiles].filter((p) => newPaths.has(p))
      );

      updateTab(tab.id, (t) => ({
        ...t,
        manifest: newManifest,
        isRefreshing: false,
        viewedFiles: preservedViewed,
        selectedFile:
          t.selectedFile && newPaths.has(t.selectedFile.path)
            ? newManifest.files.find((f) => f.path === t.selectedFile!.path) ?? newManifest.files[0] ?? null
            : newManifest.files[0] ?? null,
        commentThreads: { status: "idle" },
      }));

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
    if (!tab || tab.isRefreshing) return;

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
    updateTab(activeTabId,(t) => {
      const next = new Set(t.viewedFiles);
      if (next.has(filePath)) {
        next.delete(filePath);
      } else {
        next.add(filePath);
      }
      return { ...t, viewedFiles: next };
    });
  }

  function handleViewChange(view: SidebarView) {
    updateTab(activeTabId,(t) => ({ ...t, sidebarView: view }));
    if (view === "comments") {
      handleRequestComments();
    }
  }

  async function handleRequestComments() {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab || tab.commentThreads.status === "loading" || tab.commentThreads.status === "loaded") return;

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
    if (!tab || tab.commentThreads.status !== "loaded") return;

    // Optimistic update: add a placeholder comment
    const optimisticComment = {
      id: `optimistic-${Date.now()}`,
      body,
      author: { login: "you", avatar_url: "" },
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
      url: "",
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

  async function handleSubmitReview(event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT", body: string) {
    const tab = tabs.find((t) => t.id === activeTabId);
    if (!tab) return;

    try {
      await invoke<string>("submit_review", {
        prUrl: tab.manifest.pr_url,
        event,
        body,
      });
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
    if (!tab) return;

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
    const next = tabs.filter((t) => t.id !== tabId);
    setTabs(next);
    if (tabId === activeTabId) {
      if (next.length === 0) {
        setActiveTabId(null);
      } else {
        const newIdx = Math.min(idx, next.length - 1);
        setActiveTabId(next[newIdx].id);
      }
    }
  }

  useEffect(() => {
    const interval = setInterval(async () => {
      const currentTabs = tabsRef.current;
      if (loadingRef.current || currentTabs.length === 0) return;

      const pollableTabs = currentTabs.filter((t) => !t.isRefreshing);
      await Promise.allSettled(
        pollableTabs.map(async (tab) => {
          const status = await invoke<PrUpdateStatus>("check_pr_updates", {
            prUrl: tab.manifest.pr_url,
            currentHeadSha: tab.manifest.head_sha,
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
      </div>
    );
  }

  if (tabs.length === 0 || showOpener) {
    return (
      <div
        className="app empty-state"
        onDragOver={(e) => e.preventDefault()}
        onDrop={handleFileDrop}
      >
        <div className="empty-message">
          {loading ? (
            <LoadingView
              prRef={loadingPrRef}
              prTitle={loadingPrTitle}
              progress={progress}
              fileCounts={fileCounts}
              onCancel={handleFetchCancel}
            />
          ) : (
            <>
              <h1>Relevant Reviews</h1>
              <p>
                Drop a manifest JSON file here, or enter a PR URL below to start
                a review.
              </p>
              <PrOpener onFetchStart={handleFetchStart} onSettingsClick={() => setSettingsOpen(true)} />
              <ReviewRequestList onSelectPr={handleFetchStart} />
            </>
          )}
          {tabs.length > 0 && !loading && (
            <button
              className="settings-button"
              onClick={() => setShowOpener(false)}
              style={{ marginTop: 16 }}
            >
              Cancel
            </button>
          )}
        </div>
        <SettingsModal
          open={settingsOpen}
          onClose={() => setSettingsOpen(false)}
        />
      </div>
    );
  }

  return (
    <div className="app">
      <Header
        tabs={tabs}
        activeTabId={activeTabId}
        onSelectTab={setActiveTabId}
        onCloseTab={closeTab}
        onNewReview={() => setShowOpener(true)}
        viewMode={viewMode}
        onViewModeChange={setViewMode}
        viewedCount={activeTab?.viewedFiles.size ?? 0}
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
      />
      <SettingsModal
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
      />
      {activeTab && (
        <>
        <SearchBar
          files={activeTab.manifest.files}
          selectedFile={activeTab.selectedFile}
          onSelectFile={setSelectedFile}
          onHighlightMatches={(matches, idx, q) => { setSearchMatches(matches); setSearchCurrentIndex(idx); setSearchQuery(q); }}
          onClearHighlights={() => { setSearchMatches([]); setSearchCurrentIndex(0); setSearchQuery(""); }}
        />
        <div className="main-content">
          <FileSidebar
            files={activeTab.manifest.files}
            changeGroups={activeTab.manifest.change_groups ?? []}
            selectedFile={activeTab.selectedFile}
            onSelectFile={setSelectedFile}
            viewedFiles={activeTab.viewedFiles}
            onToggleViewed={toggleViewed}
            showHunkSignificance={showHunkSignificance}
            hunkFilter={hunkFilter}
            onHunkFilterChange={setHunkFilter}
            sidebarView={activeTab.sidebarView}
            onViewChange={handleViewChange}
            commentThreads={activeTab.commentThreads.status === "loaded" ? activeTab.commentThreads.threads : []}
            selectedCommentFile={activeTab.selectedCommentFile}
            onSelectCommentFile={handleSelectCommentFile}
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
                  lang={activeTab.selectedCommentFile ? detectLanguage(activeTab.selectedCommentFile) : undefined}
                />
              ) : (
                <div className="no-file-selected">Switch to Comments tab to load threads</div>
              )
            ) : activeTab.selectedFile ? (
              <DiffViewer key={activeTab.selectedFile.path} file={activeTab.selectedFile} viewMode={viewMode} showHunkSignificance={showHunkSignificance} showAiNotes={showAiNotes} onCreateComment={handleCreateComment} onEditComment={handleEditComment} reviewThreads={activeTab.commentThreads.status === "loaded" ? activeTab.commentThreads.threads : undefined} searchMatches={fileSearchMatches} currentSearchMatch={currentSearchMatch} searchQuery={searchQuery} />
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
        </>
      )}
      <ToastContainer toasts={toasts} onDismiss={removeToast} />
    </div>
  );
}

export default App;
