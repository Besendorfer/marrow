import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { open as openUrl } from "@tauri-apps/plugin-shell";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { PrActivityItem, Settings, Watch } from "../types";
import { useActivityFeed, useReviewSession, prRefOf, type ReviewSession } from "../hooks/useActivityFeed";
import { timeAgo } from "../utils";

interface ActivityWidgetProps {
  /** Open a PR in the main window (an `owner/repo#number` ref). */
  onOpenPr: (prRef: string) => void;
  /**
   * `dock` = the in-app, resizable, fixed-corner panel (shown while you're in
   * Marrow). `window` = the floating NSPanel (shown while you're away).
   */
  variant?: "dock" | "window";
}

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
    default:
      return { glyph: "●", cls: "aw-ci--pending" };
  }
}

/** Animated equalizer bars — the Spotify nod, shown on the pill and header. */
function Equalizer() {
  return (
    <span className="aw-eq" aria-hidden>
      <i />
      <i />
      <i />
    </span>
  );
}

type StoryTone = "urgent" | "bad" | "good" | "neutral";

interface Story {
  text: string;
  tone: StoryTone;
}

/**
 * Compose the row's one-line "story" sentence, priority-ordered. Replaces the
 * old deltas/reasons chip row with a single sentence built from item fields.
 *
 * Note: `is_re_requested` (which the backend uses for `tier`/`urgency`) isn't
 * part of `PrActivityItem`'s wire shape — the frontend only ever sees its
 * *effect* (tier === "needs_you", high urgency), not the flag itself. There's
 * no delta/reason string that distinguishes "re-requested after changes" from
 * an ordinary review request either, so that branch is intentionally skipped.
 */
function storyFor(item: PrActivityItem): Story {
  const hasDelta = (d: string) => item.deltas.includes(d);
  const isOwn = item.tier === "yours";

  if (isOwn && (item.ciState === "failure" || item.ciState === "error")) {
    return {
      text: hasDelta("new-commits") ? "CI went red after new commits" : "CI went red",
      tone: "bad",
    };
  }
  if (isOwn && item.reviewState === "approved" && item.ciState === "success") {
    return { text: "Approved · CI green — ready to merge", tone: "good" };
  }
  if (hasDelta("new-comments")) {
    const n = item.unresolvedThreads;
    return {
      text: n && n > 0 ? `New comments · ${n} unresolved thread${n === 1 ? "" : "s"}` : "New comments",
      tone: "neutral",
    };
  }
  if (item.reasons.includes("review-requested")) {
    const n = item.unresolvedThreads;
    return {
      text:
        n && n > 0
          ? `Review requested by ${item.author} · ${n} unresolved thread${n === 1 ? "" : "s"}`
          : `Review requested by ${item.author}`,
      tone: "urgent",
    };
  }
  // needs_you via a notification (mention / team review request) — cover the
  // same reasons core's is_review_ish_reason grants the tier for, so these
  // rows don't fall through to the generic "Updated Xm" fallback.
  if (item.tier === "needs_you") {
    const mention = item.reasons.some((r) => r.startsWith("notification:") && r.includes("mention"));
    return {
      text: mention ? `You were mentioned by ${item.author}` : `Your review was requested`,
      tone: "urgent",
    };
  }
  if (hasDelta("new-commits")) return { text: "New commits", tone: "neutral" };
  if (hasDelta("ci-changed")) return { text: `CI ${item.ciState ?? "changed"}`, tone: "neutral" };
  // The watch-label fallback only makes sense in the watching tier — under
  // YOUR PRS / NEEDS YOU a "watching: X" line reads like a mis-filed row.
  const watching = item.tier === "watching" ? item.reasons.find((r) => r.startsWith("watching:")) : undefined;
  if (watching) {
    return {
      text: `watching: ${watching.slice("watching:".length)} · updated ${timeAgo(item.updatedAt, true)}`,
      tone: "neutral",
    };
  }
  return { text: `Updated ${timeAgo(item.updatedAt, true)}`, tone: "neutral" };
}

/** Fallback glyph for the pill ticker when the item has no CI status to show. */
function toneGlyph(tone: StoryTone): string {
  switch (tone) {
    case "bad":
      return "✗";
    case "good":
      return "✓";
    default:
      return "●";
  }
}

function tickerTextFor(item: PrActivityItem, needsYouCount: number): string {
  const story = storyFor(item);
  const ci = ciGlyph(item.ciState);
  const glyph = ci?.glyph ?? toneGlyph(story.tone);
  return `${glyph} ${story.text} on ${item.repo}#${item.number} · ${needsYouCount} need you`;
}

/**
 * Open a PR's GitHub URL in the user's browser. The dock renders inside the
 * main window, which has the `shell:default` capability, so it can call the
 * shell plugin directly (same pattern as Header.tsx/App.tsx). The floating
 * window's own webview has no shell capability (see
 * `app-tauri/capabilities/mini-player.json`) and none can be added here, so it
 * routes through a plain frontend event instead — App.tsx listens for
 * `aw-open-external` and opens the URL on the main window's behalf.
 */
function openOnGithub(url: string, variant: "dock" | "window") {
  if (variant === "dock") {
    openUrl(url).catch(() => {});
  } else {
    emit("aw-open-external", url).catch(() => {});
  }
}

const TIER_ORDER = ["needs_you", "yours", "watching"] as const;
const TIER_LABELS: Record<string, string> = {
  needs_you: "Needs you",
  yours: "Your PRs",
  watching: "Watching",
};

/** Urgency desc, then unread desc, then most-recently-updated first. */
function sortSection(items: PrActivityItem[]): PrActivityItem[] {
  return [...items].sort((a, b) => {
    if (b.urgency !== a.urgency) return b.urgency - a.urgency;
    if (a.unread !== b.unread) return a.unread ? -1 : 1;
    return new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime();
  });
}

function ActivityRow({
  item,
  variant,
  onActivate,
  onMarkSeen,
  onSnooze,
  onUnsnooze,
  snoozedMode = false,
}: {
  item: PrActivityItem;
  variant: "dock" | "window";
  onActivate: (item: PrActivityItem) => void;
  onMarkSeen: (item: PrActivityItem) => void;
  onSnooze: (item: PrActivityItem) => void;
  onUnsnooze: (item: PrActivityItem) => void;
  snoozedMode?: boolean;
}) {
  const ci = ciGlyph(item.ciState);
  const story = storyFor(item);
  return (
    <div
      className={`aw-row ${item.unread ? "aw-row--unread" : ""} ${snoozedMode ? "aw-row--snoozed" : ""}`}
    >
      <button
        className="aw-row__activate"
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
            <span className={`aw-story aw-story--${story.tone}`}>{story.text}</span>
          </span>
        </span>
        <span className="aw-row__status">
          {ci && <span className={`aw-ci ${ci.cls}`}>{ci.glyph}</span>}
          {!!item.unresolvedThreads && item.unresolvedThreads > 0 && (
            <span className="aw-threads" title="unresolved threads">
              {item.unresolvedThreads}
            </span>
          )}
          <span className="aw-row__time">{timeAgo(item.updatedAt, true)}</span>
        </span>
      </button>
      <span className="aw-row__actions">
        {snoozedMode ? (
          <button
            className="aw-row__action"
            onClick={(e) => {
              e.stopPropagation();
              onUnsnooze(item);
            }}
            aria-label="Unsnooze"
            title="Unsnooze"
          >
            zz
          </button>
        ) : (
          <>
            <button
              className="aw-row__action"
              onClick={(e) => {
                e.stopPropagation();
                onMarkSeen(item);
              }}
              aria-label="Mark seen"
              title="Mark seen"
            >
              ✓
            </button>
            <button
              className="aw-row__action"
              onClick={(e) => {
                e.stopPropagation();
                onSnooze(item);
              }}
              aria-label="Snooze"
              title="Snooze"
            >
              zz
            </button>
            <button
              className="aw-row__action"
              onClick={(e) => {
                e.stopPropagation();
                openOnGithub(item.prUrl, variant);
              }}
              aria-label="Open on GitHub"
              title="Open on GitHub"
            >
              ↗
            </button>
          </>
        )}
      </span>
    </div>
  );
}

/** The "Now Reviewing" card: reflects the main window's active tab (via the
 * `review-session` event) so both mini-player variants can jump back in. */
function NowReviewingCard({
  session,
  compact,
}: {
  session: ReviewSession;
  compact: boolean;
}) {
  const pct =
    session.relevantCount > 0
      ? Math.min(100, Math.round((session.viewedCount / session.relevantCount) * 100))
      : 0;
  const nextLabel = session.nextFile ? session.nextFile.split("/").pop() : null;

  function resume() {
    // Works for both variants: the floating window round-trips through the
    // main window (as usual), and for the dock — which already IS the main
    // window — this just re-focuses itself and advances to the next
    // unviewed file, which App.tsx's `deep-link-resume` listener already
    // handles regardless of which window emitted the event.
    invoke("resume_review_in_main", { prRef: session.prRef }).catch(() => {});
  }

  return (
    <div className={`aw-now ${compact ? "aw-now--compact" : ""}`}>
      <div className="aw-now__top">
        <div className="aw-now__info">
          <span className="aw-now__eyebrow">Now reviewing</span>
          <span className="aw-now__title">
            #{session.number} {session.title}
          </span>
          <span className="aw-now__meta">
            {session.viewedCount} of {session.relevantCount} files
            {nextLabel ? ` · next: ${nextLabel}` : " · all files viewed"}
          </span>
        </div>
        <button className="aw-now__resume" onClick={resume}>
          Resume
        </button>
      </div>
      <div className="aw-now__bar">
        <div className="aw-now__bar-fill" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

const COLLAPSED_KEY = "aw-collapsed";

export function ActivityWidget({ onOpenPr, variant = "dock" }: ActivityWidgetProps) {
  const { items, truncatedTotal, unreadCount, markSeen, snoozePr, unsnoozePr } = useActivityFeed();
  const session = useReviewSession();
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
  const [snoozedExpanded, setSnoozedExpanded] = useState(false);
  // Temporary feed filters (not persisted).
  const [query, setQuery] = useState("");
  const [source, setSource] = useState("all"); // a raw reason string, or "all"
  const [watchLabels, setWatchLabels] = useState<string[]>([]);
  const [showControls, setShowControls] = useState(false);
  // Whether the floating window auto-shows when Marrow is backgrounded (dock only).
  const [floatingEnabled, setFloatingEnabled] = useState(true);

  // Height tier: show the "now playing" focus bar only when the widget is too
  // short for a useful list; otherwise show the list. Height-axis container
  // queries don't match in the macOS WKWebView, so measure the height directly.
  const rootRef = useRef<HTMLDivElement>(null);
  const [compact, setCompact] = useState(false);
  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => {
      setCompact(entry.contentRect.height < 180);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [collapsed]);

  // Keep the dock's floating-window toggle in sync with the persisted setting —
  // on mount and whenever the main window regains focus (so it reflects a ✕
  // dismissal made from the floating window while we were away).
  useEffect(() => {
    if (variant !== "dock") return;
    const load = () =>
      invoke<Settings>("get_settings")
        .then((s) => setFloatingEnabled(s.activity_mini_player ?? true))
        .catch(() => {});
    load();
    const unlisten = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused) load();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [variant]);

  function toggleFloating() {
    const next = !floatingEnabled;
    setFloatingEnabled(next);
    invoke("set_mini_player_enabled", { enabled: next }).catch(() => {});
  }

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

  // Existing filters (unread-only, search, source) apply BEFORE tiering.
  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return items.filter((i) => {
      if (unreadOnly && !i.unread) return false;
      if (source !== "all" && !i.reasons.includes(source)) return false;
      if (q) {
        const hay = `${i.title} ${i.repo}#${i.number} ${i.author}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }, [items, unreadOnly, source, query]);

  // Group the filtered feed into tier sections (needs_you → yours → watching),
  // sorted within each section; snoozed items are pulled out into their own
  // trailing collapsed section instead of their tier.
  const { sections, snoozed } = useMemo(() => {
    const nonSnoozed = visible.filter((i) => !i.snoozed);
    const snoozed = visible.filter((i) => i.snoozed);
    const byTier: Record<string, PrActivityItem[]> = {};
    for (const i of nonSnoozed) (byTier[i.tier] ??= []).push(i);
    const sections = TIER_ORDER.map((tier) => ({ tier, items: sortSection(byTier[tier] ?? []) })).filter(
      (s) => s.items.length > 0
    );
    return { sections, snoozed };
  }, [visible]);

  const needsYouItems = sections.find((s) => s.tier === "needs_you")?.items ?? [];

  // Compact tier's prev/next transport steps through the needs-you tier only.
  const safeIdx = needsYouItems.length ? Math.min(focusIdx, needsYouItems.length - 1) : 0;
  const compactFocus = needsYouItems[safeIdx];

  // Pill ticker: rotates through the top 3 items by urgency (raw feed, not
  // narrowed by the search/filter controls — the pill is a global glance).
  const tickerItems = useMemo(
    () =>
      [...items]
        .filter((i) => !i.snoozed)
        .sort((a, b) => b.urgency - a.urgency)
        .slice(0, 3),
    [items]
  );
  const needsYouCount = useMemo(
    () => items.filter((i) => !i.snoozed && i.tier === "needs_you").length,
    [items]
  );
  const prefersReducedMotion = useMemo(
    () => typeof window !== "undefined" && !!window.matchMedia?.("(prefers-reduced-motion: reduce)").matches,
    []
  );
  const [tickerIdx, setTickerIdx] = useState(0);
  const [tickerVisible, setTickerVisible] = useState(true);
  const [tickerPaused, setTickerPaused] = useState(false);
  useEffect(() => {
    if (prefersReducedMotion || tickerPaused || tickerItems.length <= 1) return;
    const id = setInterval(() => {
      setTickerVisible(false);
      setTimeout(() => {
        setTickerIdx((i) => (i + 1) % tickerItems.length);
        setTickerVisible(true);
      }, 300);
    }, 6000);
    return () => clearInterval(id);
  }, [prefersReducedMotion, tickerPaused, tickerItems.length]);
  const safeTickerIdx = tickerItems.length ? Math.min(tickerIdx, tickerItems.length - 1) : 0;
  const tickerItem = tickerItems[safeTickerIdx];

  function activate(item: PrActivityItem) {
    markSeen(item);
    onOpenPr(prRefOf(item));
  }

  // Collapsed → glanceable pill (Tier 0). Only the in-app dock collapses; the
  // floating window always shows the full widget.
  if (collapsed && variant === "dock") {
    return (
      <button
        className={`activity-widget activity-widget--pill ${unreadCount ? "activity-widget--alert" : ""}`}
        onClick={() => setCollapsedPersist(false)}
        onMouseEnter={() => setTickerPaused(true)}
        onMouseLeave={() => setTickerPaused(false)}
        title="PR activity"
      >
        <Equalizer />
        <span className={`aw-ticker__text ${tickerVisible ? "" : "aw-ticker__text--hidden"}`}>
          {tickerItem ? tickerTextFor(tickerItem, needsYouCount) : "All caught up"}
        </span>
      </button>
    );
  }

  return (
    <div
      ref={rootRef}
      className={`activity-widget activity-widget--${variant} ${compact ? "aw--compact" : ""}`}
    >
      <header className="aw-head" data-tauri-drag-region>
        <Equalizer />
        <span className="aw-head__title">
          {variant === "window" ? "Marrow Activity" : "Activity"}
        </span>
        {unreadCount > 0 && <span className="aw-head__badge">{unreadCount}</span>}
        <span className="aw-head__spacer" />
        {/* Interactive controls work in BOTH variants now: the floating panel
            hides only when the MAIN window gains focus, and clicking these
            focuses the panel, not main — so they don't dismiss it. */}
        <button
          className={`aw-iconbtn ${unreadOnly ? "aw-iconbtn--on" : ""}`}
          onClick={() => setUnreadOnly((v) => !v)}
          title={unreadOnly ? "Showing unread" : "Showing all"}
        >
          {unreadOnly ? "Unread" : "All"}
        </button>
        <button
          className={`aw-iconbtn ${showControls || query.trim() || source !== "all" ? "aw-iconbtn--on" : ""}`}
          onClick={() => setShowControls((v) => !v)}
          title="Search & filter"
        >
          ⌕
        </button>
        {variant === "dock" && (
          <>
            <button
              className={`aw-iconbtn ${floatingEnabled ? "aw-iconbtn--on" : ""}`}
              onClick={toggleFloating}
              title={
                floatingEnabled
                  ? "Floating window: on — shows when Marrow is in the background"
                  : "Floating window: off"
              }
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
            onClick={() => invoke("dismiss_mini_player").catch(() => {})}
            title="Dismiss (disable until re-enabled in Settings)"
          >
            ✕
          </button>
        )}
      </header>

      {/* Search + source filter — collapsed by default, toggled from the header
          (and hidden by CSS in the short "bar" layout). */}
      {showControls && (
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
      )}

      {/* "Now Reviewing" — mirrors the main window's active tab. Hidden when
          there's no active session, or (outside the compact tier) while the
          search box is open. */}
      {session && (!showControls || compact) && (
        <NowReviewingCard session={session} compact={compact} />
      )}

      {/* Compact tier: no scrolling list — just a prev/next strip through the
          needs-you tier. Driven by `.aw--compact` (toggled in JS from a
          measured height) because height-axis container queries don't match
          in the macOS WKWebView. */}
      {compact ? (
        <div className="aw-compact-foot">
          <div className="aw-compact-foot__transport">
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
              onClick={() => setFocusIdx((i) => Math.min(needsYouItems.length - 1, i + 1))}
              disabled={safeIdx >= needsYouItems.length - 1}
              title="Next"
            >
              ›
            </button>
          </div>
          {compactFocus ? (
            <span className="aw-compact-foot__next">
              next: {compactFocus.repo}#{compactFocus.number} — {storyFor(compactFocus).text}
            </span>
          ) : (
            <span className="aw-compact-foot__next aw-compact-foot__next--empty">
              Nothing needs you right now
            </span>
          )}
        </div>
      ) : (
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
            <>
              {sections.map((s) => (
                <div className="aw-tier" key={s.tier}>
                  <div className="aw-tier__head">
                    <span>{TIER_LABELS[s.tier]}</span>
                    <span className="aw-tier__count">{s.items.length}</span>
                  </div>
                  {s.items.map((item) => (
                    <ActivityRow
                      key={item.prUrl}
                      item={item}
                      variant={variant}
                      onActivate={activate}
                      onMarkSeen={markSeen}
                      onSnooze={snoozePr}
                      onUnsnooze={unsnoozePr}
                    />
                  ))}
                </div>
              ))}
              {snoozed.length > 0 && (
                <div className="aw-tier aw-tier--snoozed">
                  <button
                    className="aw-tier__head aw-tier__head--clickable"
                    onClick={() => setSnoozedExpanded((v) => !v)}
                  >
                    <span>{snoozedExpanded ? "▾" : "▸"} Snoozed</span>
                    <span className="aw-tier__count">{snoozed.length}</span>
                  </button>
                  {snoozedExpanded &&
                    snoozed.map((item) => (
                      <ActivityRow
                        key={item.prUrl}
                        item={item}
                        variant={variant}
                        snoozedMode
                        onActivate={activate}
                        onMarkSeen={markSeen}
                        onSnooze={snoozePr}
                        onUnsnooze={unsnoozePr}
                      />
                    ))}
                </div>
              )}
            </>
          )}
          {truncatedTotal > 0 && (
            <div className="aw-truncated">+{truncatedTotal} more not shown</div>
          )}
        </div>
      )}
    </div>
  );
}
