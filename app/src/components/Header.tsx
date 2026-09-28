import { useState, useRef, useEffect, useCallback } from "react";
import { open } from "@tauri-apps/plugin-shell";
import { countFailingChecks } from "../utils";
import type { ReviewManifest, Tab, MyReviewState, PrLens, PrChecksStatus } from "../types";

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
  /** Open this tab's Finish panel (issue #238 phase 5). */
  onFinishReview?: () => void;
  onRefresh?: () => void;
  isRefreshing?: boolean;
  myReviewState?: MyReviewState;
  onCheckForUpdates: () => void;
  onOpenPalette: () => void;
  chatOpen?: boolean;
  onToggleChat?: () => void;
  /** Inbox review layout (issue #238): one Review lens instead of Overview + Files. */
  inboxMode?: boolean;
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
  onFinishReview,
  onRefresh,
  isRefreshing,
  myReviewState,
  onCheckForUpdates,
  onOpenPalette,
  chatOpen,
  onToggleChat,
  inboxMode,
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
            <LensSwitcher lens={lens} onSetLens={onSetLens} filesCount={filesCount} commitsCount={commitsCount} filesProgress={progress} checksState={checksState} inboxMode={inboxMode} />
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
            {onFinishReview && (
              <button className="review-submit-toggle" onClick={onFinishReview} title="Recap, verdict, and submit (R)">
                Finish review
                {myReviewState && REVIEW_STATUS_SYMBOL[myReviewState.status] && !myReviewState.is_re_requested && (
                  <span className="review-submit-status-badge">{REVIEW_STATUS_SYMBOL[myReviewState.status]}</span>
                )}
              </button>
            )}
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
  inboxMode,
}: {
  lens: PrLens;
  onSetLens: (lens: PrLens) => void;
  filesCount: number;
  commitsCount: number;
  filesProgress: number;
  checksState: PrChecksStatus | null;
  /** Inbox layout (issue #238): Overview + Files collapse into one Review lens. */
  inboxMode?: boolean;
}) {
  const badge = checksBadge(checksState);
  return (
    <div className="lens-switcher">
      {inboxMode ? (
      <button
        className={`seg-item${lens === "overview" || lens === "files" ? " active" : ""}`}
        onClick={() => onSetLens(lens === "files" ? "files" : "overview")}
      >
        Review
      </button>
      ) : (<>
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
      </>)}
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

