import type { TriageReport } from "../types";

interface TriageCardProps {
  triage: TriageReport;
  /** Jump to a file (and line, when known) flagged as a top risk. */
  onJump: (path: string, startLine?: number | null) => void;
  /** Begin the guided fastest-path review from the first ordered file. */
  onStartGuided: () => void;
}

/**
 * Top-of-review "what changed that matters" card: the 2-3 highest-risk changes
 * with jump links, plus an entry point into the guided path. Shown when no file
 * is selected. Primes the reviewer on risk before they start reading in order.
 */
export function TriageCard({ triage, onJump, onStartGuided }: TriageCardProps) {
  const risks = triage.top_risks;
  return (
    <div className="triage-card">
      <div className="triage-header">
        <h3>What to review first</h3>
        {triage.review_order.length > 0 && (
          <button className="triage-start" onClick={onStartGuided}>
            Start guided review &rarr;
          </button>
        )}
      </div>
      {risks.length === 0 ? (
        <p className="triage-empty">
          No standout high-risk changes — use the guided path to review the files in
          the order that's easiest to follow.
        </p>
      ) : (
        <ol className="triage-risks">
          {risks.map((r, i) => (
            <li key={i} className="triage-risk">
              <button
                className="triage-risk-jump"
                onClick={() => onJump(r.path, r.start_line)}
                title={`Jump to ${r.path}${r.start_line ? `:${r.start_line}` : ""}`}
              >
                <span className="triage-risk-title">{r.title}</span>
                <span className="triage-risk-detail">{r.detail}</span>
                <span className="triage-risk-path">
                  {r.path}
                  {r.start_line ? `:${r.start_line}` : ""}
                </span>
              </button>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}
