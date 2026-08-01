import { Fragment, useEffect, useRef, useState } from "react";
import { SummaryParagraphs } from "./SummaryParagraphs";
import { Avatar } from "./ReviewRequestList";
import { RichText } from "./RichText";
import { highlightKey, timeAgo } from "../utils";
import type { ReviewManifest, FileDiff, ChangeGroup, PrChecksStatus, MyReviewState, TopRisk, PrCommit } from "../types";

interface PrOverviewProps {
  manifest: ReviewManifest;
  checksStatus?: PrChecksStatus | null;
  reviewState: MyReviewState | null;
  viewedCount: number;
  unresolvedThreads: number | null;
  hasSubmittedReview: boolean;
  startTarget: FileDiff | null;
  onStartReview: () => void;
  onSelectFile: (f: FileDiff) => void;
  /** Keys (see highlightKey) of AI highlights introduced by the most recent
   * refresh's re-analysis — surfaced as a "new AI notes" chip when non-empty. */
  newHighlightKeys?: Set<string>;
  /** Jump to a file, optionally scrolling to a specific line (head version). */
  onOpenAt?: (path: string, line?: number) => void;
  /** Start a whole-PR chat walkthrough, most-important-first. */
  onBriefMe?: () => void;
  /** Enter commit scope for a row in the Commits card. */
  onViewCommit?: (commit: PrCommit) => void;
}

/** First file (in manifest order) whose highlights include a new-note key. */
function firstNewNoteFile(manifest: ReviewManifest, newHighlightKeys: Set<string>): FileDiff | null {
  for (const f of manifest.files) {
    for (const h of f.highlights ?? []) {
      if (newHighlightKeys.has(highlightKey(f.path, h))) return f;
    }
  }
  return null;
}

function CiChip({ checks }: { checks: PrChecksStatus }) {
  // GitHub conclusions arrive uppercase ("FAILURE"); only overall_state is
  // normalized to lowercase in core.
  const failing = checks.check_runs.filter((c) => c.conclusion === "FAILURE").length;
  const { dot, label } =
    checks.overall_state === "success"
      ? { dot: "risk-dot--ok", label: "CI passing" }
      : failing > 0
        ? { dot: "risk-dot--critical", label: `${failing} CI ${failing === 1 ? "check" : "checks"} failing` }
        : { dot: "risk-dot--medium", label: "CI running" };
  return (
    <span className="overview-chip overview-chip--ci">
      <span className={`risk-dot ${dot}`} /> {label}
    </span>
  );
}

function fileName(path: string): string {
  return path.split("/").pop() ?? path;
}

/** Collapsed-by-default PR description card, rendered as minimal markdown. */
function DescriptionCard({ body }: { body: string }) {
  const [expanded, setExpanded] = useState(false);
  // Only offer the toggle when the collapsed body actually overflows — a
  // short description gets a plain card, not a do-nothing "Show more".
  const [overflows, setOverflows] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = bodyRef.current;
    if (el) setOverflows(el.scrollHeight > el.clientHeight + 1);
  }, [body]);
  return (
    <div className="overview-card overview-description">
      <h4>Description</h4>
      <div
        ref={bodyRef}
        className={`overview-description-body${expanded ? "" : " overview-description-body--collapsed"}`}
      >
        <RichText content={body} />
      </div>
      {(overflows || expanded) && (
        <button className="overview-description-toggle" onClick={() => setExpanded((v) => !v)}>
          {expanded ? "Show less" : "Show more"}
        </button>
      )}
    </div>
  );
}

function GroupRow({
  group,
  files,
  onSelectFile,
}: {
  group: ChangeGroup;
  files: FileDiff[];
  onSelectFile: (f: FileDiff) => void;
}) {
  if (files.length === 0) return null;
  const criticalNotes = files.reduce(
    (n, f) => n + (f.highlights?.filter((h) => h.severity === "critical").length ?? 0),
    0
  );
  const highRisk = files.filter(
    (f) => f.risk_level === "critical" || f.risk_level === "high"
  ).length;
  return (
    <button className="overview-group" onClick={() => onSelectFile(files[0])}>
      <div className="overview-group-main">
        <div className="overview-group-name">{group.label}</div>
        {group.description && (
          <div className="overview-group-desc">{group.description}</div>
        )}
      </div>
      <div className="overview-group-side">
        {criticalNotes > 0 && (
          <span className="risk-badge risk-critical">
            {criticalNotes} critical
          </span>
        )}
        {criticalNotes === 0 && highRisk > 0 && (
          <span className="risk-badge risk-high">{highRisk} high</span>
        )}
        <span className="overview-chip">
          {files.length} {files.length === 1 ? "file" : "files"}
        </span>
      </div>
    </button>
  );
}

/** Splits `commits` (chronological, oldest first) into the commits newer than
 * the viewer's last review and the rest, or `null` when no split should be
 * shown (no review yet, or everything/nothing is new). Falls back to
 * comparing `committed_at` when `last_reviewed_sha` isn't found in the list
 * (a force push rewrote history). */
function splitByLastReview(
  commits: PrCommit[],
  reviewState: MyReviewState | null
): { newer: PrCommit[]; older: PrCommit[] } | null {
  if (!reviewState || !reviewState.last_reviewed_sha) return null;
  const idx = commits.findIndex((c) => c.sha === reviewState.last_reviewed_sha);
  const newer =
    idx >= 0
      ? commits.slice(idx + 1)
      : reviewState.last_reviewed_at
        ? commits.filter((c) => c.committed_at > reviewState.last_reviewed_at!)
        : [];
  // All-new is still a split (header with an empty "Earlier"): after a full
  // history rewrite the "since your last review" signal matters most.
  if (newer.length === 0) return null;
  const newerShas = new Set(newer.map((c) => c.sha));
  const older = commits.filter((c) => !newerShas.has(c.sha));
  return { newer, older };
}

function CommitRow({ commit, onViewCommit }: { commit: PrCommit; onViewCommit?: (commit: PrCommit) => void }) {
  return (
    <button className="overview-commit-row" onClick={() => onViewCommit?.(commit)}>
      {commit.author_login && <Avatar login={commit.author_login} size={16} />}
      <span className="overview-commit-message">{commit.message_headline}</span>
      <span className="overview-commit-sha">{commit.sha.slice(0, 7)}</span>
      <span className="overview-commit-time">{timeAgo(commit.committed_at, true)}</span>
    </button>
  );
}

const COMMITS_COLLAPSED_LIMIT = 8;

function CommitsCard({
  commits,
  reviewState,
  onViewCommit,
}: {
  commits: PrCommit[];
  reviewState: MyReviewState | null;
  onViewCommit?: (commit: PrCommit) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const split = splitByLastReview(commits, reviewState);
  type CommitRowItem = { commit: PrCommit; group: "new" | "old" };
  const ordered: CommitRowItem[] = split
    ? ([...split.newer].reverse().map((c) => ({ commit: c, group: "new" })) as CommitRowItem[]).concat(
        [...split.older].reverse().map((c) => ({ commit: c, group: "old" }))
      )
    : [...commits].reverse().map((c) => ({ commit: c, group: "old" }));
  const visible = expanded ? ordered : ordered.slice(0, COMMITS_COLLAPSED_LIMIT);

  return (
    <div className="overview-card">
      <h4>Commits</h4>
      {visible.map((item, i) => {
        const prevGroup = i > 0 ? visible[i - 1].group : null;
        return (
          <Fragment key={item.commit.sha}>
            {split && item.group === "new" && i === 0 && (
              <div className="overview-commit-header">{split.newer.length} since your last review</div>
            )}
            {split && item.group === "old" && prevGroup === "new" && (
              <div className="overview-commit-divider">Earlier</div>
            )}
            <CommitRow commit={item.commit} onViewCommit={onViewCommit} />
          </Fragment>
        );
      })}
      {ordered.length > COMMITS_COLLAPSED_LIMIT && (
        <button className="overview-description-toggle" onClick={() => setExpanded((v) => !v)}>
          {expanded ? "Show less" : `Show all ${commits.length}`}
        </button>
      )}
    </div>
  );
}

function TopRiskRow({ risk, onOpenAt }: { risk: TopRisk; onOpenAt?: (path: string, line?: number) => void }) {
  return (
    <button
      className="overview-risk-row"
      onClick={() => onOpenAt?.(risk.path, risk.start_line ?? undefined)}
    >
      <div className="overview-risk-row-main">
        <div className="overview-risk-row-title">{risk.title}</div>
        <div className="overview-risk-row-detail">{risk.detail}</div>
      </div>
      <span className="overview-risk-file">
        {fileName(risk.path)}
        {risk.start_line ? `:${risk.start_line}` : ""}
      </span>
    </button>
  );
}

export function PrOverview({
  manifest,
  checksStatus,
  reviewState,
  viewedCount,
  unresolvedThreads,
  hasSubmittedReview,
  startTarget,
  onStartReview,
  onSelectFile,
  newHighlightKeys,
  onOpenAt,
  onBriefMe,
  onViewCommit,
}: PrOverviewProps) {
  const newNoteCount = newHighlightKeys?.size ?? 0;
  const relevant = manifest.files.filter((f) => f.classification !== "NOT_RELEVANT");
  const setAside = manifest.files.length - relevant.length;
  const byPath = new Map(manifest.files.map((f) => [f.path, f]));
  const groups = manifest.change_groups ?? [];
  const author = reviewState?.author || manifest.author;
  const draft = reviewState ? reviewState.draft : manifest.draft;
  const approvedBy = reviewState?.approved_by ?? [];
  const labels = reviewState?.labels ?? [];
  const visibleLabels = labels.slice(0, 5);
  const hiddenLabelCount = labels.length - visibleLabels.length;
  const totalAdd = manifest.files.reduce((n, f) => n + f.additions, 0);
  const totalDel = manifest.files.reduce((n, f) => n + f.deletions, 0);
  const criticalNotes = relevant.reduce(
    (n, f) => n + (f.highlights?.filter((h) => h.severity === "critical").length ?? 0),
    0
  );
  const highRisk = relevant.filter(
    (f) => f.risk_level === "critical" || f.risk_level === "high"
  ).length;
  const topRisks = manifest.triage?.top_risks ?? [];

  return (
    <div className="pr-overview">
      <div className="overview-col">
        <div className="overview-meta">
          <span className="overview-title">
            <span className="overview-title-num">#{manifest.pr_number}</span>
            {manifest.pr_title}
          </span>
          {author && (
            <span className="overview-chip overview-chip--author">
              <Avatar login={author} size={16} /> {author}
            </span>
          )}
          {draft && <span className="overview-chip overview-chip--draft">Draft</span>}
          {checksStatus && <CiChip checks={checksStatus} />}
          {newNoteCount > 0 && (
            <button
              type="button"
              className="overview-chip overview-chip--new-notes"
              onClick={() => {
                const f = firstNewNoteFile(manifest, newHighlightKeys!);
                if (f) onSelectFile(f);
              }}
            >
              <span className="overview-new-dot" /> {newNoteCount} new AI {newNoteCount === 1 ? "note" : "notes"}
            </button>
          )}
          <span className="overview-chip">
            {relevant.length} relevant of {manifest.files.length}{" "}
            {manifest.files.length === 1 ? "file" : "files"}
          </span>
          <span className="overview-chip overview-chip--branch">
            {manifest.base_ref} ← {manifest.head_ref} · +{totalAdd} −{totalDel}
          </span>
          {reviewState?.mergeable === "conflicting" && (
            <span className="overview-chip overview-chip--conflict">Has conflicts</span>
          )}
          {visibleLabels.map((label) => (
            <span key={label.name} className="overview-chip overview-chip--label">
              <span className="overview-label-dot" style={{ background: `#${label.color}` }} />
              {label.name}
            </span>
          ))}
          {hiddenLabelCount > 0 && (
            <span className="overview-chip">+{hiddenLabelCount}</span>
          )}
        </div>
        {manifest.summary && (
          <div className="overview-card">
            <h4>AI summary</h4>
            <SummaryParagraphs text={manifest.summary} />
          </div>
        )}
        {manifest.body && <DescriptionCard body={manifest.body} />}
        {groups.length > 0 && (
          <div className="overview-card">
            <h4>Change groups</h4>
            {groups.map((g, i) => (
              <GroupRow
                key={i}
                group={g}
                files={g.file_paths
                  .map((p) => byPath.get(p))
                  .filter((f): f is FileDiff => !!f)}
                onSelectFile={onSelectFile}
              />
            ))}
          </div>
        )}
        {manifest.commits.length > 0 && (
          <CommitsCard commits={manifest.commits} reviewState={reviewState} onViewCommit={onViewCommit} />
        )}
        {setAside > 0 && (
          <div className="overview-card overview-noise">
            {setAside} {setAside === 1 ? "file" : "files"} set aside as noise —
            lockfiles, generated code, and other changes the AI judged not worth
            your review time.
          </div>
        )}
      </div>
      <div className="overview-rail">
        {topRisks.length > 0 && (
          <div className="overview-card overview-risks">
            <h4>What to review first</h4>
            {topRisks.map((risk, i) => (
              <TopRiskRow key={i} risk={risk} onOpenAt={onOpenAt} />
            ))}
          </div>
        )}
        <div className="overview-card">
          <h4>Start reviewing</h4>
          <div className="overview-risk-strip">
            {criticalNotes > 0 && (
              <span>
                <span className="risk-dot risk-dot--critical" /> {criticalNotes}{" "}
                critical {criticalNotes === 1 ? "note" : "notes"}
              </span>
            )}
            {highRisk > 0 && (
              <span>
                <span className="risk-dot risk-dot--high" /> {highRisk} high-risk{" "}
                {highRisk === 1 ? "file" : "files"}
              </span>
            )}
            {criticalNotes === 0 && highRisk === 0 && (
              <span>No high-risk changes flagged</span>
            )}
          </div>
          <div className="overview-start-row">
            {startTarget ? (
              <button className="overview-start" onClick={onStartReview}>
                Start review → {fileName(startTarget.path)}
              </button>
            ) : (
              <div className="overview-state-line">All files reviewed</div>
            )}
            {onBriefMe && (
              <button className="overview-start-secondary" onClick={onBriefMe}>
                Brief me
              </button>
            )}
          </div>
          <div className="overview-start-sub">
            {relevant.length} relevant {relevant.length === 1 ? "file" : "files"},
            in review order
          </div>
        </div>
        <div className="overview-card">
          <h4>Review state</h4>
          {approvedBy.length > 0 ? (
            <div className="overview-state-line overview-approvals">
              <span className="risk-dot risk-dot--ok" /> Approved by
              <span className="overview-approvers">
                {approvedBy.map((login) => (
                  <span key={login} className="overview-approver">
                    <Avatar login={login} size={16} /> {login}
                  </span>
                ))}
              </span>
            </div>
          ) : (
            reviewState && <div className="overview-state-line">No approvals yet</div>
          )}
          <div className="overview-state-line">
            {viewedCount} of {relevant.length} files reviewed
          </div>
          <div className="overview-state-line">
            {hasSubmittedReview ? "Review submitted" : "No review submitted yet"}
          </div>
          {unresolvedThreads !== null && unresolvedThreads > 0 && (
            <div className="overview-state-line">
              {unresolvedThreads} unresolved{" "}
              {unresolvedThreads === 1 ? "thread" : "threads"}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
