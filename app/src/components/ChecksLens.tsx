import { useEffect, useState } from "react";
import { open as openUrl } from "@tauri-apps/plugin-shell";
import { countFailingChecks, getFileName, isFailingCheck } from "../utils";
import type { CheckAnnotation, CheckAnnotationsState, CheckRunInfo, PrChecksStatus } from "../types";

interface ChecksLensProps {
  checks: PrChecksStatus | null;
  annotations: CheckAnnotationsState;
  /** Paths present in this PR's diff — CI often anchors run-level failures to
   * paths (e.g. `.github`) that aren't, so those annotation cards can't jump. */
  diffPaths: Set<string>;
  /** The PR's head SHA — resets the local run-filter selection when it
   * changes (a refresh, or switching to a different PR's tab). */
  headSha: string;
  onOpenAt: (path: string, line?: number) => void;
}

function runState(run: CheckRunInfo): "ok" | "fail" | "pending" {
  if (run.status !== "COMPLETED") return "pending";
  if (isFailingCheck(run)) return "fail";
  return "ok";
}

/** Stable-sort annotations so every annotation for the same path is adjacent,
 * in first-seen path order — a lightweight "group by path" that keeps the
 * flat row list the canvas renders. Moved from PrOverview (issue #175) —
 * the failing-checks card that used to own this now lives here. */
function groupAnnotationsByPath(annotations: CheckAnnotation[]): CheckAnnotation[] {
  const order: string[] = [];
  const byPath = new Map<string, CheckAnnotation[]>();
  for (const a of annotations) {
    if (!byPath.has(a.path)) {
      byPath.set(a.path, []);
      order.push(a.path);
    }
    byPath.get(a.path)!.push(a);
  }
  return order.flatMap((p) => byPath.get(p)!);
}

function levelPillClass(level: string): string {
  if (level === "failure") return "risk-critical";
  if (level === "warning") return "risk-medium";
  return "risk-low";
}

function CheckRunRow({
  run,
  selected,
  onSelect,
}: {
  run: CheckRunInfo;
  selected: boolean;
  onSelect: () => void;
}) {
  const state = runState(run);
  const glyph = state === "ok" ? "✓" : state === "fail" ? "✗" : "●";
  return (
    <div className={`checks-lens-row${selected ? " selected" : ""}${state === "fail" ? " ck-row-fail" : ""}`}>
      <button type="button" className="checks-lens-row-main" onClick={onSelect} title={run.name}>
        <span className={`checks-lens-row-glyph ck-glyph--${state}`}>{glyph}</span>
        <span className="checks-lens-row-name">{run.name}</span>
      </button>
      {run.details_url && (
        <button
          type="button"
          className="github-link checks-lens-row-details"
          onClick={() => openUrl(run.details_url!).catch(() => {})}
        >
          Details ↗
        </button>
      )}
    </div>
  );
}

function CheckFailureCard({
  annotation,
  inDiff,
  onOpenAt,
}: {
  annotation: CheckAnnotation;
  /** Whether the annotation's path is a file in this PR's diff — CI often
   * anchors run-level failures to paths like `.github` that aren't. */
  inDiff: boolean;
  onOpenAt: (path: string, line?: number) => void;
}) {
  const title = annotation.title ?? annotation.message.split("\n")[0];
  return (
    <button
      type="button"
      className={`ck-annotation-card${inDiff ? "" : " ck-annotation-card--nodiff"}`}
      disabled={!inDiff}
      title={inDiff ? undefined : "Not a file in this PR's diff — see the check's log on GitHub"}
      onClick={() => inDiff && onOpenAt(annotation.path, annotation.start_line)}
    >
      <span className={`risk-badge ${levelPillClass(annotation.annotation_level)}`}>
        {annotation.annotation_level}
      </span>
      <div className="ck-annotation-card-main">
        <div className="ck-annotation-card-title">
          {annotation.check_name} <span className="ck-annotation-card-sep">·</span> {title}
        </div>
        <pre className="check-annotation-message check-annotation-message-collapsed">
          {annotation.message}
        </pre>
      </div>
      <span className="overview-risk-file">
        {getFileName(annotation.path)}:{annotation.start_line}
      </span>
    </button>
  );
}

/** The Checks lens (issue #175): a rail of every check run on the PR's head
 * commit, mirroring the Commits lens's rail grammar, with a canvas that
 * groups failing/warning annotations by path — reusing the pieces the old
 * Overview "Failing checks" card used to own (moved here since that card is
 * gone). Frontend-only: `checks`/`annotations` are data the app already
 * fetches (checksMap polling + the on-demand annotation fetch), no new Rust
 * calls. */
export function ChecksLens({ checks, annotations, diffPaths, headSha, onOpenAt }: ChecksLensProps) {
  // Which run the canvas is filtered to — purely presentational to this lens
  // (not persisted), reset whenever the PR head moves so a stale filter never
  // survives a refresh or a switch to a different PR's tab.
  const [selectedRun, setSelectedRun] = useState<string | null>(null);
  useEffect(() => {
    setSelectedRun(null);
  }, [headSha]);

  const totalRuns = checks?.check_runs.length ?? 0;
  const failingRuns = checks ? countFailingChecks(checks) : 0;
  // Runs still queued/in progress: without this the canvas would claim "all
  // passing" while the header badge pulses amber for the same data.
  const pendingRuns = checks?.check_runs.filter((r) => r.status !== "COMPLETED").length ?? 0;

  function selectRun(name: string) {
    setSelectedRun((cur) => (cur === name ? null : name));
  }

  return (
    <>
      <aside className="checks-lens-rail">
        <div className="checks-lens-rail-head">
          Checks <span className="checks-lens-rail-count">· {totalRuns} runs</span>
        </div>
        <div className="checks-lens-rail-list">
          {checks?.check_runs.map((run, i) => (
            <CheckRunRow key={`${run.name}-${i}`} run={run} selected={run.name === selectedRun} onSelect={() => selectRun(run.name)} />
          ))}
        </div>
      </aside>
      <div className="checks-lens-canvas">
        {annotations.status === "loading" || annotations.status === "idle" ? (
          <div className="no-file-selected">Loading checks…</div>
        ) : annotations.status === "error" ? (
          <div className="no-file-selected">{annotations.message}</div>
        ) : totalRuns === 0 ? (
          <div className="no-file-selected">No checks reported on this PR.</div>
        ) : (
          (() => {
            const allAnnotations = annotations.failures.annotations;
            const failureCount = allAnnotations.filter((a) => a.annotation_level === "failure").length;
            const warningCount = allAnnotations.filter((a) => a.annotation_level === "warning").length;
            const filtered = selectedRun ? allAnnotations.filter((a) => a.check_name === selectedRun) : allAnnotations;
            const grouped = groupAnnotationsByPath(filtered);

            return (
              <>
                <div className="checks-lens-summary">
                  <span className="checks-lens-summary-headline">
                    {failingRuns > 0
                      ? `${failingRuns} ${failingRuns === 1 ? "check" : "checks"} failing`
                      : pendingRuns > 0
                        ? `${pendingRuns} of ${totalRuns} ${pendingRuns === 1 ? "check" : "checks"} still running`
                        : `All ${totalRuns} checks passing`}
                  </span>
                  {failureCount > 0 && (
                    <span className="risk-badge risk-critical">
                      {failureCount} {failureCount === 1 ? "failure" : "failures"}
                    </span>
                  )}
                  {warningCount > 0 && (
                    <span className="risk-badge risk-medium">
                      {warningCount} {warningCount === 1 ? "warning" : "warnings"}
                    </span>
                  )}
                  {selectedRun && (
                    <button type="button" className="overview-chip ck-run-filter" onClick={() => setSelectedRun(null)}>
                      Run: {selectedRun} ✕
                    </button>
                  )}
                </div>
                {failingRuns === 0 ? (
                  <div className="overview-card ck-all-passing">
                    {pendingRuns > 0
                      ? `${pendingRuns} of ${totalRuns} ${pendingRuns === 1 ? "check is" : "checks are"} still running.`
                      : `All ${totalRuns} checks passing.`}
                  </div>
                ) : grouped.length === 0 ? (
                  <div className="no-file-selected">No annotations for this run.</div>
                ) : (
                  <div className="overview-card ck-annotations">
                    {grouped.map((a, i) => (
                      <CheckFailureCard key={i} annotation={a} inDiff={diffPaths.has(a.path)} onOpenAt={onOpenAt} />
                    ))}
                    {annotations.failures.truncated && (
                      <div className="overview-checks-truncated">Showing the first 200 annotations</div>
                    )}
                  </div>
                )}
              </>
            );
          })()
        )}
      </div>
    </>
  );
}
