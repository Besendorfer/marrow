import { useState } from "react";

interface PrOpenerProps {
  onFetchStart: (prRef: string) => void;
  onSettingsClick?: () => void;
  onCheckForUpdates?: () => void;
}

export function PrOpener({ onFetchStart, onSettingsClick, onCheckForUpdates }: PrOpenerProps) {
  const [prRef, setPrRef] = useState("");

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!prRef.trim()) return;
    onFetchStart(prRef.trim());
  }

  return (
    <div className="pr-opener">
      <form className="pr-opener-form" onSubmit={handleSubmit}>
        <input
          type="text"
          className="pr-opener-input"
          value={prRef}
          onChange={(e) => setPrRef(e.target.value)}
          placeholder="PR URL or owner/repo#123 or just #123"
        />
        <button
          type="submit"
          className="pr-opener-button"
          disabled={!prRef.trim()}
        >
          Open PR
        </button>
        {onCheckForUpdates && (
          <button
            type="button"
            className="pr-opener-gear"
            onClick={onCheckForUpdates}
            title="Check for Updates"
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
    </div>
  );
}
