import { SummaryParagraphs } from "./SummaryParagraphs";
import type { ReviewManifest, FileDiff, ChangeGroup, PrChecksStatus } from "../types";

interface PrOverviewProps {
  manifest: ReviewManifest;
  checksStatus?: PrChecksStatus | null;
  viewedCount: number;
  unresolvedThreads: number | null;
  hasSubmittedReview: boolean;
  startTarget: FileDiff | null;
  onStartReview: () => void;
  onSelectFile: (f: FileDiff) => void;
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

export function PrOverview({
  manifest,
  checksStatus,
  viewedCount,
  unresolvedThreads,
  hasSubmittedReview,
  startTarget,
  onStartReview,
  onSelectFile,
}: PrOverviewProps) {
  const relevant = manifest.files.filter((f) => f.classification !== "NOT_RELEVANT");
  const setAside = manifest.files.length - relevant.length;
  const byPath = new Map(manifest.files.map((f) => [f.path, f]));
  const groups = manifest.change_groups ?? [];
  const criticalNotes = relevant.reduce(
    (n, f) => n + (f.highlights?.filter((h) => h.severity === "critical").length ?? 0),
    0
  );
  const highRisk = relevant.filter(
    (f) => f.risk_level === "critical" || f.risk_level === "high"
  ).length;

  return (
    <div className="pr-overview">
      <div className="overview-col">
        <div className="overview-meta">
          <span className="overview-title">
            <span className="overview-title-num">#{manifest.pr_number}</span>
            {manifest.pr_title}
          </span>
          {checksStatus && <CiChip checks={checksStatus} />}
          <span className="overview-chip">
            {relevant.length} relevant of {manifest.files.length}{" "}
            {manifest.files.length === 1 ? "file" : "files"}
          </span>
        </div>
        {manifest.summary && (
          <div className="overview-card">
            <h4>AI summary</h4>
            <SummaryParagraphs text={manifest.summary} />
          </div>
        )}
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
        {setAside > 0 && (
          <div className="overview-card overview-noise">
            {setAside} {setAside === 1 ? "file" : "files"} set aside as noise —
            lockfiles, generated code, and other changes the AI judged not worth
            your review time.
          </div>
        )}
      </div>
      <div className="overview-rail">
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
          {startTarget ? (
            <button className="overview-start" onClick={onStartReview}>
              Start review → {fileName(startTarget.path)}
            </button>
          ) : (
            <div className="overview-state-line">All files reviewed</div>
          )}
          <div className="overview-start-sub">
            {relevant.length} relevant {relevant.length === 1 ? "file" : "files"},
            in review order
          </div>
        </div>
        <div className="overview-card">
          <h4>Your state</h4>
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
