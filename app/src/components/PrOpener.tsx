import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface PrOpenerProps {
  onFetchStart: (prRef: string) => void;
  onFilterChange?: (filter: string) => void;
  onSettingsClick?: () => void;
  onCheckForUpdates?: () => void;
  /** Authenticated GitHub login, once known — shown as a quiet status chip. */
  viewerLogin?: string | null;
}

// One box does both jobs: paste anything the backend parser accepts and Enter
// opens it; any other text filters the queue below as you type. "Openable" is
// decided by the real parser (check_pr_ref → pr_parser.rs), not a mirrored
// regex — the frontend copy was a fifth instance of the quadruplicated
// PR-ref pattern, flagged by Marrow's own AI review.
export function PrOpener({ onFetchStart, onFilterChange, onSettingsClick, onCheckForUpdates, viewerLogin }: PrOpenerProps) {
  const [value, setValue] = useState("");
  const [openable, setOpenable] = useState(false);
  // Guards against out-of-order IPC responses while typing.
  const checkSeq = useRef(0);

  useEffect(() => {
    const trimmed = value.trim();
    const seq = ++checkSeq.current;
    if (!trimmed) {
      setOpenable(false);
      return;
    }
    invoke<boolean>("check_pr_ref", { input: trimmed })
      .then((ok) => { if (checkSeq.current === seq) setOpenable(ok); })
      .catch(() => { if (checkSeq.current === seq) setOpenable(false); });
  }, [value]);

  // The queue filters on whatever is typed; once the parser says it's an
  // openable ref, the filter clears so the queue stays visible behind it.
  useEffect(() => {
    onFilterChange?.(openable ? "" : value.trim());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value, openable]);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = value.trim();
    if (!trimmed) return;
    // Enter is authoritative: `openable` lags one IPC roundtrip behind the
    // input, so paste + instant Enter could no-op on stale state — re-ask
    // the parser directly rather than trusting the async echo.
    const ok =
      openable ||
      (await invoke<boolean>("check_pr_ref", { input: trimmed }).catch(() => false));
    if (!ok) return;
    onFetchStart(trimmed);
    setValue("");
  }

  return (
    <form className="queue-omnibox-row" onSubmit={handleSubmit}>
      <div className="queue-omnibox">
        <span className="queue-omnibox-icon" aria-hidden="true">⌕</span>
        <input
          type="text"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder="Paste a PR URL or owner/repo#123, or filter your queue…"
        />
        {openable ? (
          <button type="submit" className="queue-omnibox-open">
            Open PR ↵
          </button>
        ) : (
          <kbd className="next-bar-key queue-omnibox-hint" title="Command palette">⌘K</kbd>
        )}
      </div>
      {viewerLogin && (
        <span className="queue-account" title="GitHub connection is working">
          <span className="queue-account-dot" /> @{viewerLogin}
        </span>
      )}
      {onCheckForUpdates && (
        <button
          type="button"
          className="pr-opener-gear"
          onClick={onCheckForUpdates}
          title="Check for updates"
          aria-label="Check for updates"
        >
          <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <path d="M17 1v6h-6" />
            <path d="M3 10a7 7 0 0 1 11.9-5l2.1 2" />
            <path d="M3 19v-6h6" />
            <path d="M17 10a7 7 0 0 1-11.9 5l-2.1-2" />
          </svg>
        </button>
      )}
      {onSettingsClick && (
        <button
          type="button"
          className="pr-opener-gear"
          onClick={onSettingsClick}
          title="Settings"
          aria-label="Settings"
        >
          <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <path d="M10 12.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Z" />
            <path d="M16.2 12.5a1.4 1.4 0 0 0 .28 1.54l.05.05a1.7 1.7 0 1 1-2.4 2.4l-.06-.05a1.4 1.4 0 0 0-1.54-.28 1.4 1.4 0 0 0-.85 1.28v.14a1.7 1.7 0 1 1-3.4 0v-.07a1.4 1.4 0 0 0-.91-1.28 1.4 1.4 0 0 0-1.54.28l-.05.05a1.7 1.7 0 1 1-2.4-2.4l.05-.06a1.4 1.4 0 0 0 .28-1.54 1.4 1.4 0 0 0-1.28-.85h-.14a1.7 1.7 0 1 1 0-3.4h.07a1.4 1.4 0 0 0 1.28-.91 1.4 1.4 0 0 0-.28-1.54l-.05-.05a1.7 1.7 0 1 1 2.4-2.4l.06.05a1.4 1.4 0 0 0 1.54.28h.07a1.4 1.4 0 0 0 .85-1.28v-.14a1.7 1.7 0 1 1 3.4 0v.07a1.4 1.4 0 0 0 .85 1.28 1.4 1.4 0 0 0 1.54-.28l.05-.05a1.7 1.7 0 1 1 2.4 2.4l-.05.06a1.4 1.4 0 0 0-.28 1.54v.07a1.4 1.4 0 0 0 1.28.85h.14a1.7 1.7 0 0 1 0 3.4h-.07a1.4 1.4 0 0 0-1.28.85Z" />
          </svg>
        </button>
      )}
    </form>
  );
}
