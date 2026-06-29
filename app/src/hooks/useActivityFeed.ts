import { useEffect, useMemo, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { PrActivityItem, PrActivityPayload } from "../types";

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

  const items = payload?.items ?? [];
  const truncated = payload?.truncated ?? {};

  const unreadCount = useMemo(() => items.filter((i) => i.unread).length, [items]);
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

  return { items, truncated, truncatedTotal, unreadCount, markSeen };
}

/** `owner/repo#number` ref used by the deep-link / fetch entry points. */
export function prRefOf(item: PrActivityItem): string {
  return `${item.owner}/${item.repo}#${item.number}`;
}
