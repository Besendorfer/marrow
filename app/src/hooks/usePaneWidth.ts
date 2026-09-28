// Resizable pane widths (issue #238 phase 7): a width in px, clamped to its
// bounds and remembered per pane in localStorage. Storage is a convenience —
// every read and write tolerates it being unavailable.
import { useCallback, useState } from "react";

export interface PaneBounds {
  min: number;
  max: number;
  initial: number;
}

export function clampWidth(width: number, { min, max }: PaneBounds): number {
  return Math.round(Math.min(max, Math.max(min, width)));
}

/** A stored width, or the default when missing, unparsable, or unreadable. */
export function readStoredWidth(raw: string | null, bounds: PaneBounds): number {
  const n = raw == null ? NaN : Number(raw);
  return Number.isFinite(n) ? clampWidth(n, bounds) : bounds.initial;
}

export function usePaneWidth(key: string, bounds: PaneBounds): [number, (width: number) => void] {
  const storageKey = `marrow.pane.${key}`;
  const [width, setWidthState] = useState(() => {
    try {
      return readStoredWidth(localStorage.getItem(storageKey), bounds);
    } catch {
      return bounds.initial;
    }
  });
  const setWidth = useCallback(
    (next: number) => {
      const w = clampWidth(next, bounds);
      setWidthState(w);
      try {
        localStorage.setItem(storageKey, String(w));
      } catch {
        // Not persisted this time; the width still applies for the session.
      }
    },
    [storageKey, bounds.min, bounds.max], // eslint-disable-line react-hooks/exhaustive-deps
  );
  return [width, setWidth];
}
