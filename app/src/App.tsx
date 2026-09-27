import { FileSidebar } from "./components/FileSidebar";
import { DiffViewer } from "./components/DiffViewer";
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
import { ChecksLens } from "./components/ChecksLens";
import { NextFileBar } from "./components/NextFileBar";
import { SearchBar } from "./components/SearchBar";
import { KeyboardHelp } from "./components/KeyboardHelp";
import { ReviewPicker } from "./components/ReviewPicker";
import { ToastContainer } from "./components/Toast";
import { CommandPalette, type PaletteCommand } from "./components/CommandPalette";
import { WelcomeSetup } from "./components/WelcomeSetup";
import { ChatPanel } from "./components/ChatPanel";
import { open as openUrl } from "@tauri-apps/plugin-shell";
import { UpdateBanner } from "./components/UpdateBanner";
import { relaunch } from "@tauri-apps/plugin-process";
import { repoBaseUrl } from "./review/helpers";
import { useReviewController } from "./review/useReviewController";

function App() {
  const {
    tabs,
    activeTabId,
    viewMode,
    setViewMode,
    showHunkSignificance,
    setShowHunkSignificance,
    showAiNotes,
    setShowAiNotes,
    hunkFilter,
    setHunkFilter,
    expandAllHunks,
    error,
    setError,
    settingsOpen,
    setSettingsOpen,
    helpOpen,
    setHelpOpen,
    commitDiffLoading,
    commitDiffError,
    commitDiff,
    commitDiffCacheRef,
    paletteOpen,
    setPaletteOpen,
    welcomeOpen,
    setWelcomeOpen,
    staleConfirm,
    setStaleConfirm,
    reviewPickerOpen,
    setReviewPickerOpen,
    setSearchOpen,
    queueFilter,
    setQueueFilter,
    viewerLogin,
    searchRef,
    diffViewerRef,
    handleVisibleFilesChange,
    toasts,
    chatActionStatuses,
    updateStatus,
    setUpdateStatus,
    currentFingerprint,
    handleSettingsClose,
    removeToast,
    checkForUpdates,
    handleDownloadUpdate,
    activeTab,
    openPrUrls,
    activeChecks,
    showChecksModal,
    filesLensCount,
    commitsLensCount,
    checkFailureCounts,
    selectedFileAnnotations,
    diffFilePaths,
    showQuitHint,
    handleOpenCachedPr,
    guidedOrder,
    triageRationale,
    nextUnviewed,
    markReviewedAndAdvance,
    selectAdjacentFile,
    toggleThreadsView,
    handleNewReview,
    handleSelectTab,
    handleDismissChecks,
    resolveHighlight,
    restoreHighlight,
    resolveSpecItem,
    restoreSpecItem,
    saveLocalRequirements,
    handleChatSend,
    handleChatStop,
    handleChatClear,
    setChatOpen,
    toggleChatOpen,
    setCommentsOpen,
    handleChatToggleWholePr,
    handleChatOpenFile,
    runChatAction,
    briefMe,
    handleFetchStart,
    handleFetchCancel,
    updateTab,
    setLens,
    handleRefreshPr,
    setSelectedFile,
    openGroup,
    toggleViewed,
    handleViewChange,
    handlePostPrComment,
    handleRequestComments,
    handleViewCommit,
    handleOpenCommentFile,
    handleJumpToThread,
    handleReply,
    handleToggleResolved,
    handleEditComment,
    handleToggleReaction,
    handleSubmitReview,
    handleCreateComment,
    closeTab,
    handleFileDrop,
  } = useReviewController();

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
      {reviewPickerOpen && activeTab?.manifest && (
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
        checksState={activeChecks ?? null}
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
          onHighlightMatches={(matches, idx, q) => {
            // Imperative channel (issue #178 perf): no state round-trip — a
            // search keystroke must never re-render the diff row tree.
            const path = activeTab?.selectedFile?.path ?? null;
            const inFile = matches.filter((m) => m.filePath === path);
            const cur = matches[idx]?.filePath === path ? matches[idx] : null;
            diffViewerRef.current?.applySearch(q, inFile, cur);
          }}
          onClearHighlights={() => diffViewerRef.current?.clearSearch()}
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
          ) : activeTab.lens === "checks" ? (
            <ChecksLens
              checks={activeChecks ?? null}
              annotations={activeTab.checkAnnotations}
              diffPaths={diffFilePaths}
              headSha={activeTab.manifest.head_sha}
              onOpenAt={handleChatOpenFile}
            />
          ) : activeTab.lens === "overview" ? (
            <PrOverview
              manifest={activeTab.manifest}
              currentFingerprint={currentFingerprint}
              checksStatus={activeChecks ?? null}
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
              onOpenChecks={() => { if (activeTabId) setLens(activeTabId, "checks"); }}
              resolvedSpecKeys={activeTab.resolvedSpecKeys}
              specResolutions={activeTab.specResolutions}
              onResolveSpec={resolveSpecItem}
              onRestoreSpec={restoreSpecItem}
              localRequirements={activeTab.localRequirements}
              analyzingRequirements={activeTab.analyzingRequirements}
              onSaveRequirements={saveLocalRequirements}
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
                    <DiffViewer ref={diffViewerRef} key={activeTab.selectedFile.path} file={activeTab.selectedFile} viewMode={viewMode} onViewModeChange={setViewMode} showHunkSignificance={showHunkSignificance} showAiNotes={showAiNotes} expandAllHunks={expandAllHunks} dismissedHighlights={activeTab.dismissedHighlights} noteResolutions={activeTab.noteResolutions} newHighlightKeys={activeTab.newHighlightKeys} onResolveHighlight={resolveHighlight} onRestoreHighlight={restoreHighlight} onCreateComment={handleCreateComment} onEditComment={handleEditComment} onReply={handleReply} onToggleResolved={handleToggleResolved} onToggleReaction={handleToggleReaction} reviewThreads={activeTab.commentThreads.status === "loaded" ? activeTab.commentThreads.threads : undefined} checkAnnotations={selectedFileAnnotations} />
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
              conversation={activeTab.prConversation ?? null}
              composeInitialBody={activeTab.prCommentDraft ?? null}
              onPostPrComment={handlePostPrComment}
              onClearComposeDraft={() => updateTab(activeTab.id, (t) => ({ ...t, prCommentDraft: null }))}
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
