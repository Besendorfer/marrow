import { useState, useRef, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-shell";
import { SummaryParagraphs } from "./SummaryParagraphs";
import type { ReviewManifest, DiffViewMode, Tab, CommentThreadsState, MyReviewState } from "../types";

function useClickOutside(
  ref: React.RefObject<HTMLElement | null>,
  isActive: boolean,
  onClose: () => void,
) {
  useEffect(() => {
    if (!isActive) return;
    function handleClick(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        onClose();
      }
    }
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, [isActive, ref, onClose]);
}

const REVIEW_VERB: Record<string, string> = {
  approved: "approved",
  changes_requested: "requested changes on",
  commented: "commented on",
};

const REVIEW_STATUS_SYMBOL: Record<string, string> = {
  approved: "✓",
  changes_requested: "✕",
};

interface HeaderProps {
  tabs: Tab[];
  activeTabId: string | null;
  onSelectTab: (id: string) => void;
  onCloseTab: (id: string) => void;
  onNewReview: () => void;
  viewMode: DiffViewMode;
  onViewModeChange: (mode: DiffViewMode) => void;
  viewedCount: number;
  staleCount: number;
  onSettingsClick: () => void;
  manifest: ReviewManifest | null;
  showHunkSignificance: boolean;
  onToggleHunkSignificance: () => void;
  showAiNotes: boolean;
  onToggleAiNotes: () => void;
  commentThreads?: CommentThreadsState;
  onSubmitReview?: (event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT", body: string) => Promise<void>;
  onRefresh?: () => void;
  isRefreshing?: boolean;
  myReviewState?: MyReviewState;
  checksBlocking?: boolean;
  onCheckForUpdates: () => void;
}

export function Header({
  tabs,
  activeTabId,
  onSelectTab,
  onCloseTab,
  onNewReview,
  viewMode,
  onViewModeChange,
  viewedCount,
  staleCount,
  onSettingsClick,
  manifest,
  showHunkSignificance,
  onToggleHunkSignificance,
  showAiNotes,
  onToggleAiNotes,
  commentThreads,
  onSubmitReview,
  onRefresh,
  isRefreshing,
  myReviewState,
  checksBlocking,
  onCheckForUpdates,
}: HeaderProps) {
  const totalCount = manifest?.files.length ?? 0;
  const progress = totalCount > 0 ? (viewedCount / totalCount) * 100 : 0;
  const [summaryExpanded, setSummaryExpanded] = useState(false);
  const hasSummary = !!manifest?.summary;

  return (
    <header className="header">
      <div className="tab-bar">
        {tabs.map((tab) => (
          <div
            key={tab.id}
            className={`tab ${tab.id === activeTabId ? "tab-active" : ""}`}
            onClick={() => onSelectTab(tab.id)}
          >
            <span className="tab-label">
              {tab.unread && (
                <span
                  className={`tab-unread${tab.error ? " tab-unread-error" : ""}`}
                  aria-label={tab.error ? "Failed to load" : "Finished loading"}
                  title={tab.error ? "Failed to load" : "Finished loading"}
                />
              )}
              {tab.manifest ? (
                <>
                  <span className="tab-pr-number">#{tab.manifest.pr_number}</span>
                  <span className="tab-title">{tab.manifest.pr_title}</span>
                </>
              ) : tab.loading ? (
                <span className="tab-title">{tab.loading.prTitle ?? tab.loading.prRef}</span>
              ) : (
                <span className="tab-title">New Review</span>
              )}
            </span>
            <button
              className="tab-close"
              onClick={(e) => {
                e.stopPropagation();
                onCloseTab(tab.id);
              }}
              title="Close tab"
              aria-label="Close tab"
            >
              &times;
            </button>
          </div>
        ))}
        <button className="tab-new" onClick={onNewReview} title="Open a new PR">
          +
        </button>
      </div>
      {manifest && (
        <div className="header-toolbar">
          <div className="header-left">
            <span className="file-count">
              {viewedCount}/{totalCount} reviewed
              {staleCount > 0 && (
                <span className="stale-badge">{staleCount} changed</span>
              )}
            </span>
            <div className="progress-bar">
              <div className="progress-fill" style={{ width: `${progress}%` }} />
            </div>
            {onRefresh && (
              <button
                className={`refresh-button${isRefreshing ? " refreshing" : ""}`}
                onClick={onRefresh}
                disabled={isRefreshing}
                title={isRefreshing ? "Refreshing..." : "Refresh PR data"}
              >
                <span className="refresh-icon">&#x21bb;</span>
                {isRefreshing ? "Refreshing" : "Refresh"}
              </button>
            )}
            {hasSummary && (
              <button
                className="summary-toggle"
                onClick={() => setSummaryExpanded((p) => !p)}
                title={summaryExpanded ? "Hide summary" : "Show summary"}
              >
                {summaryExpanded ? "Hide Summary" : "Show Summary"}
              </button>
            )}
          </div>
          <div className="header-right">
            {onSubmitReview && <ReviewSubmitButton commentThreads={commentThreads} onSubmitReview={onSubmitReview} prTitle={manifest?.pr_title ?? ""} prUrl={manifest?.pr_url ?? ""} myReviewState={myReviewState} checksBlocking={checksBlocking} />}
            <ToolbarMenu
              viewMode={viewMode}
              onViewModeChange={onViewModeChange}
              showHunkSignificance={showHunkSignificance}
              onToggleHunkSignificance={onToggleHunkSignificance}
              showAiNotes={showAiNotes}
              onToggleAiNotes={onToggleAiNotes}
              prUrl={manifest.pr_url}
              onSettingsClick={onSettingsClick}
              onCheckForUpdates={onCheckForUpdates}
            />
          </div>
        </div>
      )}
      {hasSummary && summaryExpanded && (
        <div className="header-summary">
          <SummaryParagraphs text={manifest!.summary} />
        </div>
      )}
    </header>
  );
}

function ToolbarMenu({
  viewMode,
  onViewModeChange,
  showHunkSignificance,
  onToggleHunkSignificance,
  showAiNotes,
  onToggleAiNotes,
  prUrl,
  onSettingsClick,
  onCheckForUpdates,
}: {
  viewMode: DiffViewMode;
  onViewModeChange: (mode: DiffViewMode) => void;
  showHunkSignificance: boolean;
  onToggleHunkSignificance: () => void;
  showAiNotes: boolean;
  onToggleAiNotes: () => void;
  prUrl: string;
  onSettingsClick: () => void;
  onCheckForUpdates: () => void;
}) {
  const [isOpen, setIsOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const closeMenu = useCallback(() => setIsOpen(false), []);
  useClickOutside(menuRef, isOpen, closeMenu);

  return (
    <div className="toolbar-menu-wrapper" ref={menuRef}>
      <button
        className="toolbar-menu-button"
        onClick={() => setIsOpen((v) => !v)}
        title="Menu"
      >
        ⋯
      </button>
      {isOpen && (
        <div className="toolbar-menu-dropdown">
          <div className="toolbar-menu-section">
            <span className="toolbar-menu-label">View Mode</span>
            <div className="view-toggle">
              <button
                className={viewMode === "split" ? "active" : ""}
                onClick={() => onViewModeChange("split")}
              >
                Split
              </button>
              <button
                className={viewMode === "unified" ? "active" : ""}
                onClick={() => onViewModeChange("unified")}
              >
                Unified
              </button>
            </div>
          </div>
          <div className="toolbar-menu-divider" />
          <button
            className="toolbar-menu-item"
            onClick={onToggleHunkSignificance}
          >
            <span className={`toolbar-menu-check ${showHunkSignificance ? "visible" : ""}`}>✓</span>
            Significance
          </button>
          <button
            className="toolbar-menu-item"
            onClick={onToggleAiNotes}
          >
            <span className={`toolbar-menu-check ${showAiNotes ? "visible" : ""}`}>✓</span>
            AI Notes
          </button>
          <div className="toolbar-menu-divider" />
          <button
            className="toolbar-menu-item"
            onClick={(e) => {
              e.preventDefault();
              open(prUrl);
              setIsOpen(false);
            }}
          >
            <span className="toolbar-menu-check" />
            View on GitHub
          </button>
          <button
            className="toolbar-menu-item"
            onClick={() => {
              onSettingsClick();
              setIsOpen(false);
            }}
          >
            <span className="toolbar-menu-check" />
            Settings
          </button>
          <button
            className="toolbar-menu-item"
            onClick={() => {
              onCheckForUpdates();
              setIsOpen(false);
            }}
          >
            <span className="toolbar-menu-check" />
            Check for Updates
          </button>
        </div>
      )}
    </div>
  );
}

function ReviewSubmitButton({
  commentThreads,
  onSubmitReview,
  prTitle,
  prUrl,
  myReviewState,
  checksBlocking,
}: {
  commentThreads?: CommentThreadsState;
  onSubmitReview: (event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT", body: string) => Promise<void>;
  prTitle: string;
  prUrl: string;
  myReviewState?: MyReviewState;
  checksBlocking?: boolean;
}) {
  const [isOpen, setIsOpen] = useState(false);
  const [body, setBody] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [fetchedThreads, setFetchedThreads] = useState<import("../types").ReviewThread[] | null>(null);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const closeDropdown = useCallback(() => setIsOpen(false), []);
  useClickOutside(wrapperRef, isOpen, closeDropdown);

  // Use freshly fetched threads if available, otherwise fall back to prop
  const threads = fetchedThreads ?? (commentThreads?.status === "loaded" ? commentThreads.threads : []);
  const unresolvedThreads = threads.filter((t) => !t.is_resolved);
  const unresolvedCount = unresolvedThreads.length;

  // Disable if the user has already submitted a review that hasn't been dismissed or re-requested
  const hasSubmittedReview = myReviewState != null &&
    myReviewState.status !== "pending" &&
    myReviewState.status !== "dismissed" &&
    !myReviewState.is_re_requested;

  const isDisabled = hasSubmittedReview || !!checksBlocking;
  const isMerged = myReviewState?.is_merged ?? false;

  const disabledTooltip = checksBlocking
    ? "CI checks are pending or failing"
    : hasSubmittedReview
      ? `You already ${REVIEW_VERB[myReviewState!.status] ?? "reviewed"} this PR`
      : undefined;

  async function handleOpen() {
    if (isDisabled) return;
    const wasOpen = isOpen;
    setIsOpen((v) => !v);
    if (wasOpen) return;

    setGenerating(true);
    try {
      // Always fetch fresh threads from GitHub to get accurate state
      const freshThreads = await invoke<import("../types").ReviewThread[]>("fetch_review_comments", { prUrl });
      setFetchedThreads(freshThreads);

      const unresolved = freshThreads.filter((t) => !t.is_resolved);

      const threadsJson = unresolved.length > 0
        ? JSON.stringify(unresolved.map((t) => ({
            path: t.path,
            line: t.line,
            comments: t.comments.map((c) => ({ author: c.author.login, body: c.body })),
          })))
        : "[]";

      const generated = await invoke<string>("generate_review_body", {
        threadsJson,
        prTitle,
        hasUnresolved: unresolved.length > 0,
      });
      setBody(generated);
    } catch {
      // Silently fail — user can type manually
    } finally {
      setGenerating(false);
    }
  }

  async function handleSubmit(event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT") {
    setSubmitting(true);
    try {
      await onSubmitReview(event, body);
      setBody("");
      setIsOpen(false);
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="review-submit-wrapper" ref={wrapperRef}>
      <button
        className={`review-submit-toggle${isDisabled ? " review-submit-disabled" : ""}`}
        onClick={handleOpen}
        disabled={isDisabled}
        title={disabledTooltip}
      >
        Finish Review
        {unresolvedCount > 0 && !isDisabled && (
          <span className="review-submit-badge">{unresolvedCount}</span>
        )}
        {hasSubmittedReview && (
          <span className="review-submit-status-badge">{REVIEW_STATUS_SYMBOL[myReviewState!.status] ?? "●"}</span>
        )}
      </button>
      {isOpen && (
        <div className="review-submit-dropdown">
          <textarea
            className="review-submit-body"
            value={generating ? "Generating..." : body}
            onChange={(e) => setBody(e.target.value)}
            placeholder="Leave a comment with your review (optional)"
            rows={3}
            disabled={generating}
          />
          {unresolvedCount > 0 && (
            <div className="review-submit-warning">
              {unresolvedCount} unresolved {unresolvedCount === 1 ? "thread" : "threads"}
            </div>
          )}
          <div className="review-submit-actions">
            <button
              className="review-action-comment"
              disabled={submitting || generating}
              onClick={() => handleSubmit("COMMENT")}
              title="Submit review without explicit approval or change request"
            >
              Comment
            </button>
            {!isMerged && (
              <>
                <button
                  className="review-action-approve"
                  disabled={submitting || generating}
                  onClick={() => handleSubmit("APPROVE")}
                  title="Approve this pull request"
                >
                  Approve
                </button>
                <button
                  className="review-action-request-changes"
                  disabled={submitting || generating}
                  onClick={() => handleSubmit("REQUEST_CHANGES")}
                  title="Request changes on this pull request"
                >
                  Request Changes
                </button>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
