import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { PrActivityItem, Watch } from "../types";
import { useActivityFeed, prRefOf } from "../hooks/useActivityFeed";

interface ActivityWidgetProps {
  /** Open a PR in the main window (an `owner/repo#number` ref). */
  onOpenPr: (prRef: string) => void;
  /**
   * `dock` = an in-app, resizable, fixed-corner panel.
   * `window` = fills a dedicated floating window (Phase 3).
   */
  variant?: "dock" | "window";
}

function timeAgo(dateStr: string): string {
  const then = new Date(dateStr).getTime();
  if (Number.isNaN(then)) return "";
  const mins = Math.floor((Date.now() - then) / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h`;
  const days = Math.floor(hrs / 24);
  if (days < 30) return `${days}d`;
  return `${Math.floor(days / 30)}mo`;
}

const DELTA_LABELS: Record<string, string> = {
  new: "new",
  updated: "updated",
  "new-commits": "new commits",
  "new-comments": "new comments",
  "review-state-changed": "review changed",
  "new-threads": "new threads",
  "ci-changed": "CI changed",
};

/** Strip the `watching:`/`notification:` prefix for a compact chip label. */
function reasonLabel(reason: string): string {
  if (reason.startsWith("watching:")) return reason.slice("watching:".length);
  if (reason.startsWith("notification:")) return reason.slice("notification:".length).replace(/_/g, " ");
  if (reason === "review-requested") return "review requested";
  return reason;
}

/** CI status → a single glyph + semantic class. */
function ciGlyph(state?: string | null): { glyph: string; cls: string } | null {
  if (!state) return null;
  switch (state) {
    case "success":
      return { glyph: "✓", cls: "aw-ci--ok" };
    case "failure":
    case "error":
      return { glyph: "✗", cls: "aw-ci--bad" };
    case "pending":
      return { glyph: "●", cls: "aw-ci--pending" };
    default:
      return { glyph: "●", cls: "aw-ci--pending" };
  }
}

function ActivityRow({
  item,
  onActivate,
}: {
  item: PrActivityItem;
  onActivate: (item: PrActivityItem) => void;
}) {
  const ci = ciGlyph(item.ciState);
  return (
    <button
      className={`aw-row ${item.unread ? "aw-row--unread" : ""}`}
      onClick={() => onActivate(item)}
      title={`${item.repo}#${item.number} — ${item.title}`}
    >
      <span className="aw-row__dot" aria-hidden />
      {item.avatarUrl ? (
        <img className="aw-row__avatar" src={item.avatarUrl} alt="" loading="lazy" />
      ) : (
        <span className="aw-row__avatar aw-row__avatar--blank" aria-hidden />
      )}
      <span className="aw-row__main">
        <span className="aw-row__title">{item.title}</span>
        <span className="aw-row__meta">
          <span className="aw-row__repo">
            {item.repo}#{item.number}
          </span>
          {item.deltas.slice(0, 2).map((d) => (
            <span key={d} className="aw-chip aw-chip--delta">
              {DELTA_LABELS[d] ?? d}
            </span>
          ))}
          {item.reasons.slice(0, 1).map((r) => (
            <span key={r} className="aw-chip aw-chip--reason">
              {reasonLabel(r)}
            </span>
          ))}
        </span>
      </span>
      <span className="aw-row__status">
        {ci && <span className={`aw-ci ${ci.cls}`}>{ci.glyph}</span>}
        {!!item.unresolvedThreads && item.unresolvedThreads > 0 && (
          <span className="aw-threads" title="unresolved threads">
            {item.unresolvedThreads}
          </span>
        )}
        <span className="aw-row__time">{timeAgo(item.updatedAt)}</span>
      </span>
    </button>
  );
}

const COLLAPSED_KEY = "aw-collapsed";

export function ActivityWidget({ onOpenPr, variant = "dock" }: ActivityWidgetProps) {
  const { items, truncatedTotal, unreadCount, markSeen } = useActivityFeed();
  // Default to the unobtrusive pill; remember the user's choice across launches.
  const [collapsed, setCollapsed] = useState<boolean>(() => {
    try {
      return localStorage.getItem(COLLAPSED_KEY) !== "false";
    } catch {
      return true;
    }
  });
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [focusIdx, setFocusIdx] = useState(0);
  // Session snooze: hide a PR until its `updatedAt` moves (i.e. until it changes).
  const [snoozed, setSnoozed] = useState<Record<string, string>>({});
  // Temporary feed filters (not persisted).
  const [query, setQuery] = useState("");
  const [source, setSource] = useState("all"); // a raw reason string, or "all"
  const [watchLabels, setWatchLabels] = useState<string[]>([]);

  // Load configured watches so the source dropdown lists every watch — even one
  // that currently has no activity in the feed.
  useEffect(() => {
    invoke<Watch[]>("get_watches")
      .then((ws) => setWatchLabels(ws.map((w) => w.label).filter(Boolean)))
      .catch(() => {});
  }, []);

  // Distinct sources: configured watches plus any other reasons present in the
  // feed (review-requested, notifications, …).
  const sources = useMemo(() => {
    const set = new Set<string>();
    for (const l of watchLabels) set.add(`watching:${l}`);
    for (const i of items) for (const r of i.reasons) set.add(r);
    return Array.from(set).sort();
  }, [watchLabels, items]);

  function setCollapsedPersist(v: boolean) {
    setCollapsed(v);
    try {
      localStorage.setItem(COLLAPSED_KEY, String(v));
    } catch {
      /* ignore */
    }
  }

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return items.filter((i) => {
      if (unreadOnly && !i.unread) return false;
      if (snoozed[i.prUrl] === i.updatedAt) return false;
      if (source !== "all" && !i.reasons.includes(source)) return false;
      if (q) {
        const hay = `${i.title} ${i.repo}#${i.number} ${i.author}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }, [items, unreadOnly, snoozed, source, query]);

  // The "now playing" item: clamp the cursor into range as the feed changes.
  const safeIdx = visible.length ? Math.min(focusIdx, visible.length - 1) : 0;
  const focus = visible[safeIdx];

  function activate(item: PrActivityItem) {
    markSeen(item);
    onOpenPr(prRefOf(item));
  }

  function snooze(item: PrActivityItem) {
    setSnoozed((s) => ({ ...s, [item.prUrl]: item.updatedAt }));
    setFocusIdx(0);
  }

  // Collapsed → glanceable pill (Tier 0). Only the in-app dock collapses; the
  // floating window always shows the full widget.
  if (collapsed && variant === "dock") {
    return (
      <button
        className={`activity-widget activity-widget--pill ${unreadCount ? "activity-widget--alert" : ""}`}
        onClick={() => setCollapsedPersist(false)}
        title="PR activity"
      >
        <span className="aw-eq" aria-hidden>
          <i />
          <i />
          <i />
        </span>
        <span className="aw-pill__count">{unreadCount}</span>
      </button>
    );
  }

  return (
    <div className={`activity-widget activity-widget--${variant}`}>
      <header className="aw-head" data-tauri-drag-region>
        <span className="aw-eq" aria-hidden>
          <i />
          <i />
          <i />
        </span>
        <span className="aw-head__title">
          {variant === "window" ? "Marrow Activity" : "Activity"}
        </span>
        {unreadCount > 0 && <span className="aw-head__badge">{unreadCount}</span>}
        <span className="aw-head__spacer" />
        <button
          className={`aw-iconbtn ${unreadOnly ? "aw-iconbtn--on" : ""}`}
          onClick={() => setUnreadOnly((v) => !v)}
          title={unreadOnly ? "Showing unread" : "Showing all"}
        >
          {unreadOnly ? "Unread" : "All"}
        </button>
        {variant === "dock" && (
          <>
            <button
              className="aw-iconbtn"
              onClick={() => invoke("open_activity_window").catch(() => {})}
              title="Pop out to floating window"
            >
              ⧉
            </button>
            <button className="aw-iconbtn" onClick={() => setCollapsedPersist(true)} title="Collapse">
              –
            </button>
          </>
        )}
        {variant === "window" && (
          <button
            className="aw-iconbtn"
            onClick={() => getCurrentWindow().close().catch(() => {})}
            title="Close"
          >
            ✕
          </button>
        )}
      </header>

      {/* Search + source filter (hidden by CSS in the short "bar" layout). */}
      <div className="aw-controls">
        <input
          className="aw-search"
          type="text"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setFocusIdx(0);
          }}
          placeholder="Search PRs…"
          spellCheck={false}
        />
        {sources.length > 1 && (
          <select
            className="aw-source"
            value={source}
            onChange={(e) => {
              setSource(e.target.value);
              setFocusIdx(0);
            }}
            title="Filter by source"
          >
            <option value="all">All sources</option>
            {sources.map((s) => (
              <option key={s} value={s}>
                {reasonLabel(s)}
              </option>
            ))}
          </select>
        )}
      </div>

      {/* Tier 1 — "now playing". Hidden by CSS in the tall list layout. */}
      {focus && (
        <div className="aw-focus">
          <div className="aw-focus__transport">
            <button
              className="aw-iconbtn"
              onClick={() => setFocusIdx((i) => Math.max(0, i - 1))}
              disabled={safeIdx === 0}
              title="Previous"
            >
              ‹
            </button>
            <button
              className="aw-iconbtn"
              onClick={() => setFocusIdx((i) => Math.min(visible.length - 1, i + 1))}
              disabled={safeIdx >= visible.length - 1}
              title="Next"
            >
              ›
            </button>
            <span className="aw-focus__pos">
              {safeIdx + 1}/{visible.length}
            </span>
            <span className="aw-head__spacer" />
            <button
              className="aw-iconbtn"
              onClick={() => snooze(focus)}
              title="Snooze until this PR changes"
            >
              Snooze
            </button>
          </div>
          <ActivityRow item={focus} onActivate={activate} />
        </div>
      )}

      {/* Tier 2 — the feed. */}
      <div className="aw-feed">
        {visible.length === 0 ? (
          <div className="aw-empty">
            {query.trim() || source !== "all"
              ? "No matches"
              : unreadOnly
                ? "Nothing unread"
                : "No PR activity yet"}
          </div>
        ) : (
          visible.map((item) => (
            <ActivityRow key={item.prUrl} item={item} onActivate={activate} />
          ))
        )}
        {truncatedTotal > 0 && (
          <div className="aw-truncated">+{truncatedTotal} more not shown</div>
        )}
      </div>
    </div>
  );
}
