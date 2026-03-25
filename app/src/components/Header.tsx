import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-shell";
import { SummaryParagraphs } from "./SummaryParagraphs";
import type { ReviewManifest, DiffViewMode, Tab, CommentThreadsState } from "../types";

interface HeaderProps {
  tabs: Tab[];
  activeTabId: string | null;
  onSelectTab: (id: string) => void;
  onCloseTab: (id: string) => void;
  onNewReview: () => void;
  viewMode: DiffViewMode;
  onViewModeChange: (mode: DiffViewMode) => void;
  viewedCount: number;
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
              <span className="tab-pr-number">#{tab.manifest.pr_number}</span>
              <span className="tab-title">{tab.manifest.pr_title}</span>
            </span>
            <button
              className="tab-close"
              onClick={(e) => {
                e.stopPropagation();
                onCloseTab(tab.id);
              }}
              title="Close tab"
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
            <button
              className={`significance-toggle ${showHunkSignificance ? "active" : ""}`}
              onClick={onToggleHunkSignificance}
              title={showHunkSignificance ? "Hide hunk significance scoring" : "Show hunk significance scoring"}
            >
              Significance {showHunkSignificance ? "ON" : "OFF"}
            </button>
            <button
              className={`significance-toggle ${showAiNotes ? "active" : ""}`}
              onClick={onToggleAiNotes}
              title={showAiNotes ? "Hide AI highlight notes" : "Show AI highlight notes"}
            >
              AI Notes {showAiNotes ? "ON" : "OFF"}
            </button>
            <a
              className="github-link"
              href={manifest.pr_url}
              onClick={(e) => {
                e.preventDefault();
                open(manifest.pr_url);
              }}
            >
              View on GitHub
            </a>
            <button className="settings-button" onClick={onSettingsClick}>
              Settings
            </button>
            {onSubmitReview && <ReviewSubmitButton commentThreads={commentThreads} onSubmitReview={onSubmitReview} prTitle={manifest?.pr_title ?? ""} prUrl={manifest?.pr_url ?? ""} />}
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

function ReviewSubmitButton({
  commentThreads,
  onSubmitReview,
  prTitle,
  prUrl,
}: {
  commentThreads?: CommentThreadsState;
  onSubmitReview: (event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT", body: string) => Promise<void>;
  prTitle: string;
  prUrl: string;
}) {
  const [isOpen, setIsOpen] = useState(false);
  const [body, setBody] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [fetchedThreads, setFetchedThreads] = useState<import("../types").ReviewThread[] | null>(null);

  // Use freshly fetched threads if available, otherwise fall back to prop
  const threads = fetchedThreads ?? (commentThreads?.status === "loaded" ? commentThreads.threads : []);
  const unresolvedThreads = threads.filter((t) => !t.is_resolved);
  const unresolvedCount = unresolvedThreads.length;

  async function handleOpen() {
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
    <div className="review-submit-wrapper">
      <button
        className="review-submit-toggle"
        onClick={handleOpen}
      >
        Finish Review
        {unresolvedCount > 0 && (
          <span className="review-submit-badge">{unresolvedCount}</span>
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
          </div>
        </div>
      )}
    </div>
  );
}
