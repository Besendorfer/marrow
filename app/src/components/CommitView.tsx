import { open as openUrl } from "@tauri-apps/plugin-shell";
import { Avatar } from "./ReviewRequestList";
import { timeAgo } from "../utils";
import type { CommitDiff, CommitDiffFile, PrCommit } from "../types";

interface CommitViewProps {
  commit: PrCommit;
  diff: CommitDiff | null;
  loading: boolean;
  error: string | null;
  selectedPath: string | null;
  onSelectFile: (path: string) => void;
  repoBaseUrl: string;
  onNewer: () => void;
  onOlder: () => void;
  hasNewer: boolean;
  hasOlder: boolean;
  /** "View cumulative diff" chip in the meta row — hops to the Files lens
   * (issue #170; this scope no longer has a "back" exit of its own). */
  onViewCumulativeDiff: () => void;
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

/** Splits a path into its dir (with trailing slash, empty at repo root) and
 * filename, for the dimmed-parent-dir treatment used by the main sidebar. */
function splitPath(path: string): { dir: string; name: string } {
  const i = path.lastIndexOf("/");
  return i === -1 ? { dir: "", name: path } : { dir: path.slice(0, i + 1), name: path.slice(i + 1) };
}

/** Single-letter status chip mirroring FileSidebar's diff-type badge
 * (added/removed/modified) plus renamed, which the main sidebar has no
 * equivalent for since FileDiff.diff_type never carries it. */
function statusChipLabel(status: string): string {
  switch (status) {
    case "added": return "A";
    case "removed": return "D";
    case "modified": return "M";
    case "renamed": return "R";
    default: return status.charAt(0).toUpperCase();
  }
}

function statusChipClass(status: string): string {
  switch (status) {
    case "added": return "diff-type-added";
    case "removed": return "diff-type-removed";
    case "modified": return "diff-type-modified";
    case "renamed": return "diff-type-renamed";
    default: return "";
  }
}

function CommitViewFileRow({
  file,
  selected,
  onSelect,
}: {
  file: CommitDiffFile;
  selected: boolean;
  onSelect: () => void;
}) {
  const { dir, name } = splitPath(file.path);
  return (
    <div className="file-item-wrapper">
      <button
        className={`file-item commit-view-file${selected ? " selected" : ""}`}
        onClick={onSelect}
        title={file.path}
      >
        <span className={`diff-type-badge ${statusChipClass(file.status)}`}>
          {statusChipLabel(file.status)}
        </span>
        <span className="commit-view-file-path">
          {file.previous_path ? (
            `${file.previous_path} → ${file.path}`
          ) : (
            <>
              {dir && <span className="commit-view-file-dir">{dir}</span>}
              {name}
            </>
          )}
        </span>
        <span className="line-stats">
          <span className="line-stat-add">+{file.additions}</span>
          <span className="line-stat-del">-{file.deletions}</span>
        </span>
      </button>
    </div>
  );
}

/** In-layout commit scope, dropped into `.main-content` in place of the
 * FileSidebar + diff pane for the duration of the scope (see App.tsx). Same
 * per-line patch renderer as the modal it replaced (issue #147 rework) but
 * as a sidebar file list + read-only pane rather than a floating overlay. */
export function CommitView({
  commit,
  diff,
  loading,
  error,
  selectedPath,
  onSelectFile,
  repoBaseUrl,
  onNewer,
  onOlder,
  hasNewer,
  hasOlder,
  onViewCumulativeDiff,
}: CommitViewProps) {
  const commitUrl = `${repoBaseUrl}/commit/${commit.sha}`;
  const selectedFile = diff?.files.find((f) => f.path === selectedPath) ?? null;

  return (
    <>
      <aside className="file-sidebar commit-view-sidebar">
        <div className="commit-view-identity">
          <div className="commit-view-nav">
            <button className="commit-view-nav-btn" onClick={onNewer} disabled={!hasNewer}>
              ‹ Newer
            </button>
            <button className="commit-view-nav-btn" onClick={onOlder} disabled={!hasOlder}>
              Older ›
            </button>
          </div>
          <span className="commit-view-sha">{commit.sha.slice(0, 7)}</span>
          <p className="commit-view-headline">{commit.message_headline}</p>
          <div className="commit-view-meta">
            <span className="scope-pill">◉ This commit only</span>
            <button type="button" className="overview-chip commit-view-cumulative" onClick={onViewCumulativeDiff}>
              View cumulative diff
            </button>
            {commit.author_login && (
              <span className="commit-view-author">
                <Avatar login={commit.author_login} size={16} /> {commit.author_login}
              </span>
            )}
            <span title={new Date(commit.committed_at).toLocaleString()}>
              {timeAgo(commit.committed_at)}
            </span>
          </div>
        </div>
        {diff?.truncated && (
          <div className="commit-view-banner">First 300 files shown</div>
        )}
        <div className="file-list">
          {loading ? (
            <div className="commit-view-file-list-state">Loading…</div>
          ) : error ? (
            <div className="commit-view-file-list-state">Couldn't load files</div>
          ) : diff ? (
            diff.files.map((f) => (
              <CommitViewFileRow
                key={f.path}
                file={f}
                selected={f.path === selectedPath}
                onSelect={() => onSelectFile(f.path)}
              />
            ))
          ) : null}
        </div>
      </aside>
      <div className="diff-pane commit-view-pane">
        <div className="diff-header">
          {selectedFile && (
            <>
              <span className="diff-file-path">{selectedFile.path}</span>
              <span className="line-stats">
                <span className="line-stat-add">+{selectedFile.additions}</span>
                <span className="line-stat-del">-{selectedFile.deletions}</span>
              </span>
            </>
          )}
          <button
            type="button"
            className="overview-start-secondary commit-view-github"
            onClick={() => openUrl(commitUrl).catch(() => {})}
          >
            Open commit on GitHub ↗
          </button>
        </div>
        <div className="commit-view-pane-body">
          {loading ? (
            <div className="commit-view-pane-state">Loading commit…</div>
          ) : error ? (
            <div className="commit-view-pane-state">
              <div className="commit-view-pane-error">{error}</div>
              <button
                type="button"
                className="overview-start-secondary"
                onClick={() => openUrl(commitUrl).catch(() => {})}
              >
                Open on GitHub ↗
              </button>
            </div>
          ) : selectedFile ? (
            selectedFile.patch === null ? (
              <div className="commit-view-nopatch">
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
              <pre className="commit-view-patch">
                {selectedFile.patch.split("\n").map((line, i) => (
                  <div key={i} className={`diff-line-${classifyPatchLine(line)}`}>
                    {line}
                  </div>
                ))}
              </pre>
            )
          ) : (
            <div className="no-file-selected">No files in this commit</div>
          )}
        </div>
      </div>
    </>
  );
}
