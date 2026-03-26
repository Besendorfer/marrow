import { useState, useEffect, useMemo, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ReviewRequestItem, ReviewStatus, Settings, CachedPrInfo } from "../types";

interface ReviewRequestListProps {
  onSelectPr: (prRef: string) => void;
  openPrUrls: Set<string>;
}

function formatTimeAgo(dateStr: string): string {
  const date = new Date(dateStr);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));

  if (diffDays === 0) return "today";
  if (diffDays === 1) return "1 day ago";
  if (diffDays < 30) return `${diffDays} days ago`;
  const diffMonths = Math.floor(diffDays / 30);
  if (diffMonths === 1) return "1 month ago";
  return `${diffMonths} months ago`;
}

function cutoffDateStr(): string {
  const d = new Date();
  d.setDate(d.getDate() - 30);
  return d.toISOString().split("T")[0];
}

const STATUS_LABELS: Record<ReviewStatus, string> = {
  approved: "Approved",
  changes_requested: "Changes requested",
  commented: "Commented",
  dismissed: "Dismissed",
  pending: "",
};

const STATUS_CLASSES: Record<ReviewStatus, string> = {
  approved: "review-status-approved",
  changes_requested: "review-status-changes-requested",
  commented: "review-status-commented",
  dismissed: "review-status-dismissed",
  pending: "",
};

export function ReviewRequestList({ onSelectPr, openPrUrls }: ReviewRequestListProps) {
  const [cachedPrs, setCachedPrs] = useState<CachedPrInfo[]>([]);
  const [recentItems, setRecentItems] = useState<ReviewRequestItem[]>([]);
  const [olderItems, setOlderItems] = useState<ReviewRequestItem[]>([]);
  const [recentLoading, setRecentLoading] = useState(false);
  const [olderLoading, setOlderLoading] = useState(false);
  const [recentLoaded, setRecentLoaded] = useState(false);
  const [olderLoaded, setOlderLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [showOlder, setShowOlder] = useState(true);
  const [showTeam, setShowTeam] = useState(true);
  const [settingsLoaded, setSettingsLoaded] = useState(false);

  const settingsRef = useRef<Settings | null>(null);
  const cutoff = cutoffDateStr();

  useEffect(() => {
    invoke<CachedPrInfo[]>("list_cached_prs")
      .then(setCachedPrs)
      .catch(() => {});

    invoke<Settings>("get_settings").then((s) => {
      settingsRef.current = s;
      setShowOlder(s.filter_older);
      setShowTeam(s.filter_team);
      setSettingsLoaded(true);

      fetchRecent();

      if (s.filter_older) {
        fetchOlder();
      }
    });
  }, []);

  // When older is toggled on and hasn't been loaded yet, fetch on demand
  useEffect(() => {
    if (showOlder && !olderLoaded && !olderLoading && settingsLoaded) {
      fetchOlder();
    }
  }, [showOlder, olderLoaded, olderLoading, settingsLoaded]);

  async function fetchRecent() {
    setRecentLoading(true);
    setError(null);
    try {
      const results = await invoke<ReviewRequestItem[]>("fetch_review_requests", {
        cutoffDate: cutoff,
        fetchRecent: true,
      });
      setRecentItems(results);
      setRecentLoaded(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setRecentLoading(false);
    }
  }

  async function fetchOlder() {
    setOlderLoading(true);
    try {
      const results = await invoke<ReviewRequestItem[]>("fetch_review_requests", {
        cutoffDate: cutoff,
        fetchRecent: false,
      });
      setOlderItems(results);
      setOlderLoaded(true);
    } catch {
      // Older failing silently is fine — recent is the priority
    } finally {
      setOlderLoading(false);
    }
  }

  function refreshAll() {
    setRecentLoaded(false);
    setOlderLoaded(false);
    fetchRecent();
    if (showOlder) {
      fetchOlder();
    }
  }

  const saveFilters = useCallback(
    (older: boolean, team: boolean) => {
      if (settingsRef.current) {
        const updated = { ...settingsRef.current, filter_older: older, filter_team: team };
        settingsRef.current = updated;
        invoke("save_settings", { settings: updated });
      }
    },
    []
  );

  function toggleOlder() {
    setShowOlder((v) => {
      const next = !v;
      setShowTeam((team) => { saveFilters(next, team); return team; });
      return next;
    });
  }
  function toggleTeam() {
    setShowTeam((v) => {
      const next = !v;
      setShowOlder((older) => { saveFilters(older, next); return older; });
      return next;
    });
  }

  const filteredRecent = useMemo(() => {
    return recentItems.filter((item) => {
      if (!item.direct_request && !showTeam) return false;
      return true;
    });
  }, [recentItems, showTeam]);

  const filteredOlder = useMemo(() => {
    if (!showOlder) return [];
    return olderItems.filter((item) => {
      if (!item.direct_request && !showTeam) return false;
      return true;
    });
  }, [olderItems, showOlder, showTeam]);

  const filteredCached = useMemo(() => {
    return cachedPrs.filter((pr) => !openPrUrls.has(pr.pr_url));
  }, [cachedPrs, openPrUrls]);

  const hasRecent = recentItems.length > 0;
  const hasOlder = olderItems.length > 0;
  const hasTeam = recentItems.some((i) => !i.direct_request) || olderItems.some((i) => !i.direct_request);

  const isInitialLoad = recentLoading && !recentLoaded;

  const filterBar = settingsLoaded && (recentLoaded || olderLoaded) && (hasRecent || hasOlder) && (
    <div className="review-requests-filters">
      <label className="review-requests-filter">
        <input type="checkbox" checked={showOlder} onChange={toggleOlder} />
        Older
      </label>
      {hasTeam && (
        <>
          <span className="review-requests-filter-divider" />
          <label className="review-requests-filter">
            <input type="checkbox" checked={showTeam} onChange={toggleTeam} />
            Team
          </label>
        </>
      )}
    </div>
  );

  if (isInitialLoad) {
    return (
      <div className="review-requests">
        <div className="review-requests-header">
          <span className="review-requests-title">Review Requests</span>
        </div>
        <div className="review-requests-loading">
          <div className="loading-view-spinner" />
          <span>Loading review requests...</span>
        </div>
      </div>
    );
  }

  if (error && !recentLoaded) {
    return (
      <div className="review-requests">
        <div className="review-requests-header">
          <span className="review-requests-title">Review Requests</span>
        </div>
        <div className="review-requests-error">
          <span>{error}</span>
          <button className="review-requests-retry" onClick={refreshAll}>
            Retry
          </button>
        </div>
      </div>
    );
  }

  if (recentLoaded && !hasRecent && olderLoaded && !hasOlder) {
    return (
      <div className="review-requests">
        <div className="review-requests-header">
          <span className="review-requests-title">Review Requests</span>
          <button className="review-requests-refresh" onClick={refreshAll}>Refresh</button>
        </div>
        <div className="review-requests-empty">No pending review requests</div>
      </div>
    );
  }

  if (!recentLoaded && !olderLoaded) return null;

  const noFilterResults = filteredRecent.length === 0 && filteredOlder.length === 0 && !olderLoading;

  return (
    <div className="review-requests">
      <div className="review-requests-header">
        <span className="review-requests-title">Review Requests</span>
        {filterBar}
        <button className="review-requests-refresh" onClick={refreshAll}>Refresh</button>
      </div>
      <div className="review-requests-list">
        {filteredCached.length > 0 && (
          <CachedPrSection items={filteredCached} onSelectPr={onSelectPr} />
        )}
        {noFilterResults ? (
          <div className="review-requests-empty">No results match current filters</div>
        ) : (
          <>
            {filteredRecent.length > 0 && (
              <ReviewRequestSection
                label="Last 30 days"
                items={filteredRecent}
                onSelectPr={onSelectPr}
                showGroupHeaders={showTeam && filteredRecent.some((i) => i.direct_request) && filteredRecent.some((i) => !i.direct_request)}
              />
            )}
            {showOlder && (
              olderLoading ? (
                <div className="review-requests-section">
                  <div className="review-requests-section-header">
                    <span className="review-requests-section-label">Older</span>
                  </div>
                  <div className="review-requests-loading review-requests-loading-inline">
                    <div className="loading-view-spinner" />
                    <span>Loading older requests...</span>
                  </div>
                </div>
              ) : filteredOlder.length > 0 ? (
                <ReviewRequestSection
                  label="Older"
                  items={filteredOlder}
                  onSelectPr={onSelectPr}
                  showGroupHeaders={showTeam && filteredOlder.some((i) => i.direct_request) && filteredOlder.some((i) => !i.direct_request)}
                />
              ) : null
            )}
          </>
        )}
      </div>
    </div>
  );
}

function ReviewRequestSection({
  label,
  items,
  onSelectPr,
  showGroupHeaders,
}: {
  label: string;
  items: ReviewRequestItem[];
  onSelectPr: (prRef: string) => void;
  showGroupHeaders: boolean;
}) {
  const directRequests = items.filter((i) => i.direct_request);
  const teamRequests = items.filter((i) => !i.direct_request);

  return (
    <div className="review-requests-section">
      <div className="review-requests-section-header">
        <span className="review-requests-section-label">{label}</span>
        <span className="review-requests-group-count">{items.length}</span>
      </div>
      {directRequests.length > 0 && (
        <div className="review-requests-group">
          {showGroupHeaders && (
            <div className="review-requests-group-header">
              <span className="review-requests-group-label">Direct requests</span>
              <span className="review-requests-group-count">{directRequests.length}</span>
            </div>
          )}
          {directRequests.map((item) => (
            <ReviewRequestRow key={item.html_url} item={item} onSelect={onSelectPr} />
          ))}
        </div>
      )}
      {teamRequests.length > 0 && (
        <div className="review-requests-group">
          {showGroupHeaders && (
            <div className="review-requests-group-header">
              <span className="review-requests-group-label">Team requests</span>
              <span className="review-requests-group-count">{teamRequests.length}</span>
            </div>
          )}
          {teamRequests.map((item) => (
            <ReviewRequestRow key={item.html_url} item={item} onSelect={onSelectPr} />
          ))}
        </div>
      )}
    </div>
  );
}

function ReviewRequestRow({
  item,
  onSelect,
}: {
  item: ReviewRequestItem;
  onSelect: (prRef: string) => void;
}) {
  const prRef = `${item.owner}/${item.repo}#${item.number}`;
  const statusLabel = STATUS_LABELS[item.my_review_status];
  const statusClass = STATUS_CLASSES[item.my_review_status];

  return (
    <button
      className="review-request-item"
      onClick={() => onSelect(prRef)}
    >
      <div className="review-request-item-top">
        <span className="review-request-repo">{item.owner}/{item.repo}</span>
        <span className="review-request-number">#{item.number}</span>
        {statusLabel && (
          <span className={`review-status-badge ${statusClass}`}>{statusLabel}</span>
        )}
        {item.unresolved_thread_count > 0 && (
          <span className="review-threads-badge">
            {item.unresolved_thread_count} unresolved
          </span>
        )}
      </div>
      <div className="review-request-title">{item.title}</div>
      <div className="review-request-meta">
        <span className="review-request-author">{item.author}</span>
        <span className="review-request-time">{formatTimeAgo(item.created_at)}</span>
      </div>
    </button>
  );
}

function CachedPrSection({
  items,
  onSelectPr,
}: {
  items: CachedPrInfo[];
  onSelectPr: (prRef: string) => void;
}) {
  const [collapsed, setCollapsed] = useState(false);

  return (
    <div className="review-requests-section cached-prs-section">
      <button
        className="review-requests-section-header cached-prs-header"
        onClick={() => setCollapsed((v) => !v)}
      >
        <span className={`collapse-chevron ${collapsed ? "collapsed" : ""}`}>&#9662;</span>
        <span className="review-requests-section-label">Recently Analyzed</span>
        <span className="review-requests-group-count">{items.length}</span>
      </button>
      {!collapsed && (
        <div className="review-requests-group">
          {items.map((pr) => {
            const prRef = `${pr.owner}/${pr.repo}#${pr.pr_number}`;
            return (
              <button
                key={pr.pr_url}
                className="review-request-item cached-pr-item"
                onClick={() => onSelectPr(prRef)}
              >
                <div className="review-request-item-top">
                  <span className="review-request-repo">{pr.owner}/{pr.repo}</span>
                  <span className="review-request-number">#{pr.pr_number}</span>
                  <span className="cached-badge" title="Loads instantly from cache">&#9889;</span>
                </div>
                <div className="review-request-title">{pr.pr_title}</div>
                <div className="review-request-meta">
                  <span className="review-request-author">{pr.file_count} file{pr.file_count !== 1 ? "s" : ""}</span>
                  <span className="review-request-time">{formatTimeAgo(pr.cached_at)}</span>
                </div>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
