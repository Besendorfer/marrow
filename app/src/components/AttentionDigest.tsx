import type { DigestEntry } from "../types";

function fileName(path: string): string {
  return path.split("/").pop() ?? path;
}

function DigestRow({
  entry,
  onOpenAt,
  onOpenChecks,
}: {
  entry: DigestEntry;
  onOpenAt?: (path: string, line?: number) => void;
  onOpenChecks?: () => void;
}) {
  function jump() {
    if (entry.jump.kind === "file") onOpenAt?.(entry.jump.path, entry.jump.line ?? undefined);
    else if (entry.jump.kind === "checks") onOpenChecks?.();
    else onOpenChecks?.(); // MVP: url jumps fall back to the Checks lens.
  }
  return (
    <button className="overview-digest-row" onClick={jump}>
      <span className={`digest-dot digest-dot-${entry.severity}`} />
      <div className="overview-digest-row-main">
        <span className="overview-digest-row-claim">{entry.claim}</span>
        {entry.detail && <span className="overview-digest-row-detail">{entry.detail}</span>}
      </div>
      {entry.jump.kind === "file" && (
        <span className="overview-risk-file">
          {fileName(entry.jump.path)}
          {entry.jump.line ? `:${entry.jump.line}` : ""}
        </span>
      )}
    </button>
  );
}

/** Unified "needs your attention" digest (issue #180): merges CI failures and
 * triage top-risks into one ranked list, replacing the old top-risks-only
 * card. Report by exception — when nothing needs attention it collapses to
 * one quiet line instead of itemizing healthy signals. */
export function AttentionDigest({
  entries,
  allClear,
  onOpenAt,
  onOpenChecks,
}: {
  entries: DigestEntry[];
  allClear: string[];
  onOpenAt?: (path: string, line?: number) => void;
  onOpenChecks?: () => void;
}) {
  if (entries.length > 0) {
    return (
      <div className="overview-card">
        <h4>Needs your attention</h4>
        {entries.map((entry) => (
          <DigestRow key={entry.id} entry={entry} onOpenAt={onOpenAt} onOpenChecks={onOpenChecks} />
        ))}
      </div>
    );
  }
  if (allClear.length > 0) {
    return <div className="overview-allclear">✓ {allClear.join(" · ")}</div>;
  }
  return null;
}
