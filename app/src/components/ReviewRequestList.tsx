import { useState, useEffect, useMemo, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ReviewRequestItem, ReviewStatus, Settings, CachedPrInfo } from "../types";
import { timeAgo } from "../utils";

interface ReviewRequestListProps {
  onSelectPr: (prRef: string) => void;
  /** Cached rows pass their cache info so the app can warn before an
   *  unexpected AI re-analysis when the PR has moved on. */
  onSelectCachedPr?: (prRef: string, info: CachedPrInfo) => void;
  openPrUrls: Set<string>;
  /** Text filter from the omnibox — matches repo, title, and author. */
  filter?: string;
}

export function initials(login: string): string {
  return login.slice(0, 2).toUpperCase();
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

export function ReviewRequestList({ onSelectPr, onSelectCachedPr, openPrUrls, filter }: ReviewRequestListProps) {
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

  const needle = (filter ?? "").trim().toLowerCase();
  const matchesFilter = useCallback(
    (repo: string, title: string, author: string) =>
      needle === "" ||
      repo.toLowerCase().includes(needle) ||
      title.toLowerCase().includes(needle) ||
      author.toLowerCase().includes(needle),
    [needle]
  );

  const filteredRecent = useMemo(() => {
    return recentItems.filter((item) => {
      if (!item.direct_request && !showTeam) return false;
      return matchesFilter(`${item.owner}/${item.repo}`, item.title, item.author);
    });
  }, [recentItems, showTeam, matchesFilter]);

  const filteredOlder = useMemo(() => {
    if (!showOlder) return [];
    return olderItems.filter((item) => {
      if (!item.direct_request && !showTeam) return false;
      return matchesFilter(`${item.owner}/${item.repo}`, item.title, item.author);
    });
  }, [olderItems, showOlder, showTeam, matchesFilter]);

  const filteredCached = useMemo(() => {
    return cachedPrs.filter(
      (pr) =>
        !openPrUrls.has(pr.pr_url) &&
        matchesFilter(`${pr.owner}/${pr.repo}`, pr.pr_title, "")
    );
  }, [cachedPrs, openPrUrls, matchesFilter]);

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
          <span className="review-requests-title">Review queue</span>
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
          <span className="review-requests-title">Review queue</span>
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
          <span className="review-requests-title">Review queue</span>
          <button className="review-requests-refresh" onClick={refreshAll}>Refresh</button>
        </div>
        <div className="review-requests-empty">You're all caught up — no PRs are waiting on your review.</div>
      </div>
    );
  }

  if (!recentLoaded && !olderLoaded) return null;

  const noFilterResults = filteredRecent.length === 0 && filteredOlder.length === 0 && !olderLoading;

  return (
    <div className="review-requests">
      <div className="review-requests-header">
        <span className="review-requests-title">Review queue</span>
        {filterBar}
        <button className="review-requests-refresh" onClick={refreshAll}>Refresh</button>
      </div>
      <div className="review-requests-list">
        {filteredCached.length > 0 && (
          <CachedPrSection items={filteredCached} onSelectPr={onSelectPr} onSelectCachedPr={onSelectCachedPr} />
        )}
        {noFilterResults ? (
          <div className="review-requests-empty">Nothing in the queue matches.</div>
        ) : (
          <>
            {filteredRecent.length > 0 && (
              <ReviewRequestSection
                label="Needs your review"
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
    <button className={`queue-row${item.draft ? " queue-row--draft" : ""}`} onClick={() => onSelect(prRef)}>
      <span className="queue-avatar" aria-hidden="true">{initials(item.author)}</span>
      <span className="queue-main">
        <span className="queue-title">
          <span className="queue-repo">{item.owner}/{item.repo}#{item.number}</span>
          {item.title}
        </span>
        <span className="queue-meta">
          <span className="queue-why">
            {item.direct_request ? "Review requested" : "Team review request"} by {item.author}
          </span>
          {item.draft && <span className="queue-draft-chip">Draft</span>}
          {statusLabel && (
            <span className={`review-status-badge ${statusClass}`}>{statusLabel}</span>
          )}
          {item.unresolved_thread_count > 0 && (
            <span className="review-threads-badge">
              {item.unresolved_thread_count} unresolved
            </span>
          )}
        </span>
      </span>
      <span className="queue-time">{timeAgo(item.created_at, true)}</span>
      <span className="queue-review-btn">Review</span>
    </button>
  );
}

function CachedPrSection({
  items,
  onSelectPr,
  onSelectCachedPr,
}: {
  items: CachedPrInfo[];
  onSelectPr: (prRef: string) => void;
  onSelectCachedPr?: (prRef: string, info: CachedPrInfo) => void;
}) {
  const [collapsed, setCollapsed] = useState(false);

  return (
    <div className="review-requests-section cached-prs-section">
      <button
        className="review-requests-section-header cached-prs-header"
        onClick={() => setCollapsed((v) => !v)}
      >
        <span className={`collapse-chevron ${collapsed ? "collapsed" : ""}`}>&#9662;</span>
        <span className="review-requests-section-label">Recently analyzed</span>
        <span className="review-requests-group-count">{items.length}</span>
      </button>
      {!collapsed && (
        <div className="review-requests-group">
          {items.map((pr) => {
            const prRef = `${pr.owner}/${pr.repo}#${pr.pr_number}`;
            return (
              <button
                key={pr.pr_url}
                className="queue-row queue-row--dim"
                onClick={() => (onSelectCachedPr ? onSelectCachedPr(prRef, pr) : onSelectPr(prRef))}
              >
                <span className="queue-avatar" aria-hidden="true">{initials(pr.repo)}</span>
                <span className="queue-main">
                  <span className="queue-title">
                    <span className="queue-repo">{pr.owner}/{pr.repo}#{pr.pr_number}</span>
                    {pr.pr_title}
                  </span>
                  <span className="queue-meta">
                    <span className="queue-why">
                      Analyzed · {pr.file_count} file{pr.file_count !== 1 ? "s" : ""} · opens instantly
                    </span>
                  </span>
                </span>
                <span className="queue-time">{timeAgo(pr.cached_at, true)}</span>
                <span className="queue-review-btn">Open</span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
