import { useState, useRef, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-shell";
import { countFailingChecks } from "../utils";
import type { ReviewManifest, Tab, CommentThreadsState, MyReviewState, PrLens, PrChecksStatus } from "../types";

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
  viewedCount: number;
  /** Which PR lens (Overview/Files/Commits/Checks, issue #170, #175) the switcher shows active. */
  lens: PrLens;
  onSetLens: (lens: PrLens) => void;
  /** Files segment count — relevant files, falling back to total when 0 relevant. */
  filesCount: number;
  commitsCount: number;
  /** Checks segment status, from App's checksMap — null before the first fetch
   * resolves (issue #175). */
  checksState: PrChecksStatus | null;
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
  onOpenPalette: () => void;
  chatOpen?: boolean;
  onToggleChat?: () => void;
}

export function Header({
  tabs,
  activeTabId,
  onSelectTab,
  onCloseTab,
  onNewReview,
  viewedCount,
  lens,
  onSetLens,
  filesCount,
  commitsCount,
  checksState,
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
  onOpenPalette,
  chatOpen,
  onToggleChat,
}: HeaderProps) {
  const totalCount = manifest?.files.length ?? 0;
  const progress = totalCount > 0 ? (viewedCount / totalCount) * 100 : 0;

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
                <span className="tab-title">New review</span>
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
            {(myReviewState ? myReviewState.draft : manifest.draft) && !myReviewState?.is_merged && (
              <span className="pr-badge pr-badge--draft" title="This PR is a draft">Draft</span>
            )}
            {myReviewState?.is_merged && (
              <span className="pr-badge pr-badge--merged" title="This PR has been merged">
                Merged
              </span>
            )}
            {myReviewState?.status === "approved" && (
              <span
                className="pr-badge pr-badge--approved"
                title="You have approved this PR"
              >
                {REVIEW_STATUS_SYMBOL.approved} Approved
              </span>
            )}
            <LensSwitcher lens={lens} onSetLens={onSetLens} filesCount={filesCount} commitsCount={commitsCount} filesProgress={progress} checksState={checksState} />
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
          </div>
          <div className="header-right">
            {onToggleChat && (
              <button
                className={`chat-toggle${chatOpen ? " active" : ""}`}
                onClick={onToggleChat}
                title="Ask the AI about this change (⌘/Ctrl+J)"
              >
                Ask AI
              </button>
            )}
            {onSubmitReview && <ReviewSubmitButton commentThreads={commentThreads} onSubmitReview={onSubmitReview} prTitle={manifest?.pr_title ?? ""} prUrl={manifest?.pr_url ?? ""} myReviewState={myReviewState} checksBlocking={checksBlocking} />}
            <ToolbarMenu
              onOpenPalette={onOpenPalette}
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
    </header>
  );
}

/** The PR-view lens switcher (issue #170) — same segmented-control grammar as
 * the Split/Unified toggle, sized for the header. Files absorbs the old
 * header progress bar as a hairline under its label; Overview no longer
 * disappears when you dive into a file, and the "← Overview" escape hatch
 * dissolves into this. */
/** The Checks segment's badge: no data yet → none; any failing run wins over
 * a still-running one; all complete and none failing → the passing check
 * mark. Single spot this precedence is decided (issue #175). */
function checksBadge(checksState: PrChecksStatus | null): { kind: "ok" | "pending" | "fail"; failing: number } | null {
  if (!checksState) return null;
  // Zero runs is "nothing reported", not "passing" — no badge, like no data.
  if (checksState.check_runs.length === 0) return null;
  const failing = countFailingChecks(checksState);
  if (failing > 0) return { kind: "fail", failing };
  if (checksState.check_runs.some((r) => r.status !== "COMPLETED")) return { kind: "pending", failing: 0 };
  return { kind: "ok", failing: 0 };
}

function LensSwitcher({
  lens,
  onSetLens,
  filesCount,
  commitsCount,
  filesProgress,
  checksState,
}: {
  lens: PrLens;
  onSetLens: (lens: PrLens) => void;
  filesCount: number;
  commitsCount: number;
  filesProgress: number;
  checksState: PrChecksStatus | null;
}) {
  const badge = checksBadge(checksState);
  return (
    <div className="lens-switcher">
      <button
        className={`seg-item${lens === "overview" ? " active" : ""}`}
        onClick={() => onSetLens("overview")}
      >
        Overview
      </button>
      <button
        className={`seg-item${lens === "files" ? " active" : ""}`}
        onClick={() => onSetLens("files")}
      >
        Files <span className="seg-count">{filesCount}</span>
        <span className="seg-progress">
          <span className="seg-progress-fill" style={{ width: `${filesProgress}%` }} />
        </span>
      </button>
      <button
        className={`seg-item${lens === "commits" ? " active" : ""}`}
        onClick={() => onSetLens("commits")}
      >
        Commits <span className="seg-count">{commitsCount}</span>
      </button>
      <button
        className={`seg-item${lens === "checks" ? " active" : ""}`}
        onClick={() => onSetLens("checks")}
      >
        Checks
        {badge?.kind === "ok" && <span className="seg-count seg-ok">✓</span>}
        {badge?.kind === "pending" && <span className="seg-count seg-pulse">●</span>}
        {badge?.kind === "fail" && <span className="seg-count seg-fail">{badge.failing} ✗</span>}
      </button>
    </div>
  );
}

function ToolbarMenu({
  onOpenPalette,
  showHunkSignificance,
  onToggleHunkSignificance,
  showAiNotes,
  onToggleAiNotes,
  prUrl,
  onSettingsClick,
  onCheckForUpdates,
}: {
  onOpenPalette: () => void;
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
              onOpenPalette();
              setIsOpen(false);
            }}
          >
            <span className="toolbar-menu-check" />
            Command palette
            <span className="toolbar-menu-hint">⌘K</span>
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
            Check for updates
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
        Finish review
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
                  Request changes
                </button>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
