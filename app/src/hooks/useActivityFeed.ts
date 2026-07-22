import { useEffect, useMemo, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { PrActivityItem, PrActivityPayload } from "../types";

// Stable empties so the no-data render doesn't hand fresh references to
// downstream memos (which would defeat their memoization until the first event).
const EMPTY_ITEMS: PrActivityItem[] = [];
const EMPTY_TRUNCATED: Record<string, number> = {};

/**
 * Subscribe to the Rust-owned `pr-activity` event and expose the de-duplicated,
 * pre-sorted feed plus the actions both the dock and floating widget need. The
 * backend is the single source of truth, so every window that calls this hook
 * stays in sync automatically.
 */
export function useActivityFeed() {
  const [payload, setPayload] = useState<PrActivityPayload | null>(null);

  useEffect(() => {
    const unlisten = listen<PrActivityPayload>("pr-activity", (event) => {
      setPayload(event.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const items = payload?.items ?? EMPTY_ITEMS;
  const truncated = payload?.truncated ?? EMPTY_TRUNCATED;

  // Snoozed items don't count toward the badge/pulse — that's the point of
  // snoozing; they re-count automatically when a delta wakes them.
  const unreadCount = useMemo(() => items.filter((i) => i.unread && !i.snoozed).length, [items]);
  const truncatedTotal = useMemo(
    () => Object.values(truncated).reduce((a, b) => a + b, 0),
    [truncated]
  );

  /**
   * Acknowledge a PR: persist its current observable state so the backend's
   * next poll no longer flags it unread. Optimistically clears the local flag
   * too, so the UI responds instantly without waiting for the next event.
   */
  const markSeen = useCallback((item: PrActivityItem) => {
    invoke("mark_pr_seen", {
      prUrl: item.prUrl,
      observed: {
        updated_at: item.updatedAt,
        review_state: item.reviewState ?? null,
        unresolved_threads: item.unresolvedThreads ?? null,
        ci_state: item.ciState ?? null,
      },
    }).catch(() => {});
    setPayload((prev) =>
      prev
        ? {
            ...prev,
            items: prev.items.map((i) =>
              i.prUrl === item.prUrl ? { ...i, unread: false, deltas: [] } : i
            ),
          }
        : prev
    );
  }, []);

  /** Snooze a PR: mute it (backend wakes it on the next delta) and optimistically
   * flip its local `snoozed` flag so the collapsed section reflects it instantly. */
  const snoozePr = useCallback((item: PrActivityItem) => {
    invoke("snooze_pr", {
      prUrl: item.prUrl,
      observed: {
        updated_at: item.updatedAt,
        review_state: item.reviewState ?? null,
        unresolved_threads: item.unresolvedThreads ?? null,
        ci_state: item.ciState ?? null,
      },
    }).catch(() => {});
    setPayload((prev) =>
      prev
        ? {
            ...prev,
            items: prev.items.map((i) =>
              i.prUrl === item.prUrl ? { ...i, snoozed: true } : i
            ),
          }
        : prev
    );
  }, []);

  /** Clear a manual snooze without waiting for a delta. */
  const unsnoozePr = useCallback((item: PrActivityItem) => {
    invoke("unsnooze_pr", { prUrl: item.prUrl }).catch(() => {});
    setPayload((prev) =>
      prev
        ? {
            ...prev,
            items: prev.items.map((i) =>
              i.prUrl === item.prUrl ? { ...i, snoozed: false } : i
            ),
          }
        : prev
    );
  }, []);

  return { items, truncatedTotal, unreadCount, markSeen, snoozePr, unsnoozePr };
}

/** `owner/repo#number` ref used by the deep-link / fetch entry points. */
export function prRefOf(item: PrActivityItem): string {
  return `${item.owner}/${item.repo}#${item.number}`;
}

/** Payload of the `review-session` event, broadcast by the main window
 * whenever the active tab's manifest or viewed-files set changes; null when
 * no PR is active (e.g. the queue home). Mirrors the inline shape built in
 * App.tsx — kept local here since it's not part of the Rust-owned wire types. */
export interface ReviewSession {
  prUrl: string;
  prRef: string;
  number: number;
  title: string;
  viewedCount: number;
  relevantCount: number;
  nextFile: string | null;
}

/** Subscribe to the "Now Reviewing" session broadcast for the mini-player's
 * top card. Frontend-only event (no Rust command involved) — every window
 * just listens for the main window's emit. */
export function useReviewSession(): ReviewSession | null {
  const [session, setSession] = useState<ReviewSession | null>(null);

  useEffect(() => {
    const unlisten = listen<ReviewSession | null>("review-session", (event) => {
      setSession(event.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  return session;
}
