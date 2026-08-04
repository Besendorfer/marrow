import { useEffect, useState } from "react";
import { CommitView } from "./CommitView";
import { timeAgo } from "../utils";
import type { CommitDiff, PrCommit } from "../types";

interface CommitsLensProps {
  /** This PR's commits, oldest first (same order/data as the overview card). */
  commits: PrCommit[];
  /** The commit the right pane is scoped to; null only transiently (e.g. a
   * session restored straight into this lens before the auto-select effect
   * below runs). */
  selectedCommit: PrCommit | null;
  diff: CommitDiff | null;
  loading: boolean;
  error: string | null;
  /** Session cache of already-fetched commit diffs (App-level, keyed by sha)
   * — read opportunistically for the rail's +/− stats, which PrCommit itself
   * doesn't carry. A row shows no stats until its diff has been fetched once. */
  commitDiffCache: Map<string, CommitDiff>;
  repoBaseUrl: string;
  onSelectCommit: (commit: PrCommit) => void;
  onViewCumulativeDiff: () => void;
}

/** Sum of +/− across a fetched commit diff's files, or null when the diff
 * hasn't been fetched (and so isn't in the cache) yet. */
function cachedStats(cache: Map<string, CommitDiff>, sha: string): { additions: number; deletions: number } | null {
  const diff = cache.get(sha);
  if (!diff) return null;
  return diff.files.reduce(
    (acc, f) => ({ additions: acc.additions + f.additions, deletions: acc.deletions + f.deletions }),
    { additions: 0, deletions: 0 },
  );
}

function CommitRow({
  commit,
  selected,
  stats,
  onSelect,
}: {
  commit: PrCommit;
  selected: boolean;
  stats: { additions: number; deletions: number } | null;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      className={`commits-lens-row${selected ? " selected" : ""}`}
      onClick={onSelect}
      title={commit.message_headline}
    >
      <span className="commits-lens-row-msg">{commit.message_headline}</span>
      <span className="commits-lens-row-meta">
        <span className="commits-lens-row-sha">{commit.sha.slice(0, 7)}</span>
        {commit.author_login && <span>{commit.author_login}</span>}
        <span>{timeAgo(commit.committed_at, true)}</span>
        {stats && (
          <span className="commits-lens-row-stats">
            <span className="line-stat-add">+{stats.additions}</span>{" "}
            <span className="line-stat-del">−{stats.deletions}</span>
          </span>
        )}
      </span>
    </button>
  );
}

/** The Commits lens (issue #170): a rail of every commit in the PR (the
 * sidebar's quiet-row grammar) driving the existing per-commit CommitView as
 * its main pane. Replaces the old "commit scope" that dropped in over the
 * sidebar + diff pane — this is a persistent, first-class lens instead. */
export function CommitsLens({
  commits,
  selectedCommit,
  diff,
  loading,
  error,
  commitDiffCache,
  repoBaseUrl,
  onSelectCommit,
  onViewCumulativeDiff,
}: CommitsLensProps) {
  // Which file within the selected commit's diff the main pane shows — purely
  // presentational to this lens, so it lives here rather than on Tab or App.
  const [selectedPath, setSelectedPath] = useState<string | null>(null);

  useEffect(() => {
    setSelectedPath(diff?.files[0]?.path ?? null);
  }, [diff]);

  // Land on a commit even if this lens was entered without one selected yet
  // (e.g. a restored session) — mirrors setLens's auto-select for a fresh switch.
  useEffect(() => {
    if (!selectedCommit && commits.length > 0) {
      onSelectCommit(commits[commits.length - 1]);
    }
  }, [selectedCommit, commits, onSelectCommit]);

  return (
    <>
      <aside className="commits-lens-rail">
        <div className="commits-lens-rail-head">
          Commits <span className="commits-lens-rail-count">· {commits.length}</span>
        </div>
        <div className="commits-lens-rail-list">
          {commits.map((commit) => (
            <CommitRow
              key={commit.sha}
              commit={commit}
              selected={commit.sha === selectedCommit?.sha}
              stats={cachedStats(commitDiffCache, commit.sha)}
              onSelect={() => onSelectCommit(commit)}
            />
          ))}
        </div>
      </aside>
      {selectedCommit && (
        <CommitView
          commit={selectedCommit}
          diff={diff}
          loading={loading}
          error={error}
          selectedPath={selectedPath}
          onSelectFile={setSelectedPath}
          repoBaseUrl={repoBaseUrl}
          onNewer={() => {
            const idx = commits.findIndex((c) => c.sha === selectedCommit.sha);
            const n = commits[idx + 1];
            if (n) onSelectCommit(n);
          }}
          onOlder={() => {
            const idx = commits.findIndex((c) => c.sha === selectedCommit.sha);
            const o = commits[idx - 1];
            if (o) onSelectCommit(o);
          }}
          hasNewer={commits.findIndex((c) => c.sha === selectedCommit.sha) < commits.length - 1}
          hasOlder={commits.findIndex((c) => c.sha === selectedCommit.sha) > 0}
          onViewCumulativeDiff={onViewCumulativeDiff}
        />
      )}
      {!selectedCommit && (
        <div className="diff-pane commit-view-pane">
          <div className="no-file-selected">No commits fetched for this PR.</div>
        </div>
      )}
    </>
  );
}
