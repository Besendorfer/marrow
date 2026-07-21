import { useState } from "react";
import { isOpenablePrRef } from "../utils";

interface PrOpenerProps {
  onFetchStart: (prRef: string) => void;
  onFilterChange?: (filter: string) => void;
  onSettingsClick?: () => void;
  onCheckForUpdates?: () => void;
  /** Authenticated GitHub login, once known — shown as a quiet status chip. */
  viewerLogin?: string | null;
}

// One box does both jobs: paste anything that parses as a PR ref and Enter
// opens it; any other text filters the queue below as you type.
export function PrOpener({ onFetchStart, onFilterChange, onSettingsClick, onCheckForUpdates, viewerLogin }: PrOpenerProps) {
  const [value, setValue] = useState("");
  const openable = isOpenablePrRef(value.trim());

  function handleChange(v: string) {
    setValue(v);
    onFilterChange?.(isOpenablePrRef(v.trim()) ? "" : v.trim());
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = value.trim();
    if (!trimmed || !openable) return;
    onFetchStart(trimmed);
    handleChange("");
  }

  return (
    <form className="queue-omnibox-row" onSubmit={handleSubmit}>
      <div className="queue-omnibox">
        <span className="queue-omnibox-icon" aria-hidden="true">⌕</span>
        <input
          type="text"
          value={value}
          onChange={(e) => handleChange(e.target.value)}
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
