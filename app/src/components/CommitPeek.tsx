import { useEffect } from "react";
import { open as openUrl } from "@tauri-apps/plugin-shell";
import { Avatar } from "./ReviewRequestList";
import { timeAgo } from "../utils";
import type { CommitDiff, CommitDiffFile, PrCommit } from "../types";

interface CommitPeekProps {
  commit: PrCommit;
  diff: CommitDiff | null;
  loading: boolean;
  error: string | null;
  repoBaseUrl: string;
  onClose: () => void;
}

/** Classifies a raw patch line by its leading character, mapping onto the
 * same `diff-line-*` classes/tokens DiffViewer uses for hunk headers,
 * additions, removals, and context. */
function classifyPatchLine(line: string): "header" | "add" | "remove" | "context" {
  if (line.startsWith("@@")) return "header";
  if (line.startsWith("+")) return "add";
  if (line.startsWith("-")) return "remove";
  return "context";
}

function CommitPeekFile({ file, commitUrl }: { file: CommitDiffFile; commitUrl: string }) {
  return (
    <div className="commit-peek-file">
      <div className="commit-peek-file-header">
        <span className="commit-peek-file-path">
          {file.previous_path ? `${file.previous_path} → ${file.path}` : file.path}
        </span>
        <span className="commit-peek-file-stats">
          <span className="commit-peek-file-add">+{file.additions}</span>{" "}
          <span className="commit-peek-file-del">−{file.deletions}</span>
        </span>
        <span className="commit-peek-file-status">{file.status}</span>
      </div>
      {file.patch === null ? (
        <div className="commit-peek-file-nopatch">
          Diff too large or binary —{" "}
          <button
            type="button"
            className="github-link"
            onClick={() => openUrl(commitUrl).catch(() => {})}
          >
            view on GitHub
          </button>
        </div>
      ) : (
        <pre className="commit-peek-patch">
          {file.patch.split("\n").map((line, i) => (
            <div key={i} className={`diff-line-${classifyPatchLine(line)}`}>
              {line}
            </div>
          ))}
        </pre>
      )}
    </div>
  );
}

/** Read-only commit-diff overlay, opened from the Commits card on the PR
 * overview. Same backdrop / Esc-to-close / click-outside-to-close pattern as
 * ReviewPicker/KeyboardHelp — no comments, viewed-state, or AI affordances. */
export function CommitPeek({ commit, diff, loading, error, repoBaseUrl, onClose }: CommitPeekProps) {
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  const commitUrl = `${repoBaseUrl}/commit/${commit.sha}`;

  return (
    <div className="commit-peek-overlay" onClick={onClose}>
      <div className="commit-peek-modal" onClick={(e) => e.stopPropagation()}>
        <div className="commit-peek-header">
          <div className="commit-peek-header-main">
            <span className="commit-peek-sha">{commit.sha.slice(0, 7)}</span>
            <span className="commit-peek-headline">{commit.message_headline}</span>
            <div className="commit-peek-meta">
              {commit.author_login && (
                <span className="commit-peek-author">
                  <Avatar login={commit.author_login} size={16} /> {commit.author_login}
                </span>
              )}
              <span title={new Date(commit.committed_at).toLocaleString()}>
                {timeAgo(commit.committed_at)}
              </span>
            </div>
          </div>
          <button
            type="button"
            className="overview-start-secondary"
            onClick={() => openUrl(commitUrl).catch(() => {})}
          >
            Open on GitHub ↗
          </button>
          <button className="settings-close" onClick={onClose} aria-label="Close commit view">
            &times;
          </button>
        </div>
        <div className="commit-peek-body">
          {loading ? (
            <div className="commit-peek-state">Loading commit…</div>
          ) : error ? (
            <div className="commit-peek-state">
              <div className="commit-peek-error">{error}</div>
              <button
                type="button"
                className="overview-start-secondary"
                onClick={() => openUrl(commitUrl).catch(() => {})}
              >
                Open on GitHub ↗
              </button>
            </div>
          ) : diff ? (
            <>
              {diff.truncated && (
                <div className="commit-peek-banner">
                  Showing the first 300 files — open on GitHub for the full commit.
                </div>
              )}
              {diff.files.map((f) => (
                <CommitPeekFile key={f.path} file={f} commitUrl={commitUrl} />
              ))}
            </>
          ) : null}
        </div>
      </div>
    </div>
  );
}
