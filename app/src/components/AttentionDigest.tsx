import { useState } from "react";
import type { DigestEntry } from "../types";
import { DIGEST_SOURCE_LABEL, countBySource } from "./digest";

// Fixed tab order, independent of entries.length ordering — CI first, then
// risk, then spec, matching buildDigestEntries's own ranking.
const TAB_ORDER: DigestEntry["source"][] = ["ci", "triage", "coverage"];
const TAB_LABEL: Record<DigestEntry["source"], string> = { ci: "CI", triage: "Risks", coverage: "Spec" };

function fileName(path: string): string {
  return path.split("/").pop() ?? path;
}

function jumpLabel(entry: DigestEntry): string | null {
  if (entry.jump.kind === "file") {
    const line = entry.jump.line ? `:${entry.jump.line}` : "";
    return `Open ${fileName(entry.jump.path)}${line}`;
  }
  if (entry.jump.kind === "checks" || entry.jump.kind === "url") return "Open checks";
  if (entry.jump.kind === "requirements") return "Open requirements";
  return null;
}

// One interaction model for every row: click toggles the full text open
// (clamped when collapsed), and navigation/resolution are explicit action
// buttons inside the expanded row — never the row click itself.
function DigestRow({
  entry,
  expanded,
  onToggle,
  onOpenAt,
  onOpenChecks,
  onOpenRequirements,
  onResolveSpec,
  onRestoreSpec,
  resolved,
}: {
  entry: DigestEntry;
  expanded: boolean;
  onToggle: () => void;
  onOpenAt?: (path: string, line?: number) => void;
  onOpenChecks?: () => void;
  onOpenRequirements?: () => void;
  onResolveSpec?: (key: string) => void;
  onRestoreSpec?: (key: string) => void;
  /** Rendered in the resolved-items tray: muted, Restore-only, no Open/Mark
   * addressed (there's nothing left to do but bring it back). */
  resolved?: boolean;
}) {
  function jump() {
    if (entry.jump.kind === "file") onOpenAt?.(entry.jump.path, entry.jump.line ?? undefined);
    else if (entry.jump.kind === "checks") onOpenChecks?.();
    else if (entry.jump.kind === "url") onOpenChecks?.(); // MVP: url jumps fall back to the Checks lens.
    else if (entry.jump.kind === "requirements") onOpenRequirements?.();
  }
  const label = resolved ? null : jumpLabel(entry);
  const canResolve = !resolved && !!entry.resolveKey && !!onResolveSpec;
  const canRestore = !!resolved && !!entry.resolveKey && !!onRestoreSpec;
  const rowClass = ["overview-digest-row", expanded && "expanded", resolved && "resolved"]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={rowClass}>
      <button className="overview-digest-row-toggle" aria-expanded={expanded} onClick={onToggle}>
        <span className={`digest-dot digest-dot-${entry.severity}`} />
        <div className="overview-digest-row-main">
          <div className="overview-digest-row-top">
            <span className="overview-digest-row-source">{DIGEST_SOURCE_LABEL[entry.source]}</span>
            <span className="overview-digest-row-claim">{entry.claim}</span>
          </div>
          {entry.detail && <span className="overview-digest-row-detail">{entry.detail}</span>}
        </div>
      </button>
      {expanded && (label || canResolve || canRestore) && (
        <div className="overview-digest-row-actions">
          {label && (
            <button className="overview-digest-row-action" onClick={jump}>
              {label} →
            </button>
          )}
          {canResolve && (
            <button className="overview-digest-row-action" onClick={() => onResolveSpec!(entry.resolveKey!)}>
              Mark addressed ✓
            </button>
          )}
          {canRestore && (
            <button className="overview-digest-row-action" onClick={() => onRestoreSpec!(entry.resolveKey!)}>
              Restore
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function specItemCount(n: number): string {
  return `${n} spec item${n === 1 ? "" : "s"} addressed`;
}

/** Unified "needs your attention" digest (issue #180): merges CI failures and
 * triage top-risks into one ranked list, replacing the old top-risks-only
 * card. Report by exception — when nothing needs attention it collapses to
 * one quiet line instead of itemizing healthy signals.
 *
 * Coverage ("spec") rows are resolvable (issue #179 follow-up): a user can
 * mark an uncovered/partial requirement or orphan test addressed, which
 * hides it here and moves it into a "Show resolved" tray. Resolution is a
 * user acknowledgment, not a re-judgment of coverage — it never touches the
 * all-clear "requirements covered" fragment, which reflects the AI's
 * coverage verdict alone. */
export function AttentionDigest({
  entries,
  allClear,
  onOpenAt,
  onOpenChecks,
  onOpenRequirements,
  resolvedSpecKeys,
  onResolveSpec,
  onRestoreSpec,
}: {
  entries: DigestEntry[];
  allClear: string[];
  onOpenAt?: (path: string, line?: number) => void;
  onOpenChecks?: () => void;
  /** Jump target for the aggregate coverage entry (issue #179 phase 2) — scrolls
   * the RequirementsCard into view. */
  onOpenRequirements?: () => void;
  resolvedSpecKeys?: Set<string>;
  onResolveSpec?: (key: string) => void;
  onRestoreSpec?: (key: string) => void;
}) {
  const [tab, setTab] = useState<"all" | DigestEntry["source"]>("all");
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [showResolved, setShowResolved] = useState(false);

  const active = entries.filter((e) => !e.resolveKey || !resolvedSpecKeys?.has(e.resolveKey));
  const resolved = entries.filter((e) => e.resolveKey && resolvedSpecKeys?.has(e.resolveKey));

  const resolvedToggle =
    resolved.length > 0 ? (
      <button className="overview-digest-showresolved" onClick={() => setShowResolved((s) => !s)}>
        {showResolved ? "Hide resolved" : `Show resolved (${resolved.length})`}
      </button>
    ) : null;

  const resolvedRows = showResolved
    ? resolved.map((entry) => (
        <DigestRow
          key={entry.id}
          entry={entry}
          resolved
          expanded={expandedId === entry.id}
          onToggle={() => setExpandedId(expandedId === entry.id ? null : entry.id)}
          onRestoreSpec={onRestoreSpec}
        />
      ))
    : null;

  if (active.length > 0) {
    const counts = countBySource(active);
    const sources = TAB_ORDER.filter((source) => counts[source]);
    // Fall back to "all" if the selected tab's source has since emptied out,
    // rather than setState-during-render.
    const effectiveTab = tab !== "all" && !counts[tab] ? "all" : tab;
    const visibleEntries = effectiveTab === "all" ? active : active.filter((e) => e.source === effectiveTab);
    return (
      <div className="overview-card">
        <h4>Needs your attention</h4>
        {sources.length >= 2 && (
          <div className="overview-digest-tabs">
            <button
              className={effectiveTab === "all" ? "overview-digest-tab active" : "overview-digest-tab"}
              onClick={() => setTab("all")}
            >
              All ({active.length})
            </button>
            {sources.map((source) => (
              <button
                key={source}
                className={effectiveTab === source ? "overview-digest-tab active" : "overview-digest-tab"}
                onClick={() => setTab(source)}
              >
                {TAB_LABEL[source]} ({counts[source]})
              </button>
            ))}
          </div>
        )}
        {visibleEntries.map((entry) => (
          <DigestRow
            key={entry.id}
            entry={entry}
            expanded={expandedId === entry.id}
            onToggle={() => setExpandedId(expandedId === entry.id ? null : entry.id)}
            onOpenAt={onOpenAt}
            onOpenChecks={onOpenChecks}
            onOpenRequirements={onOpenRequirements}
            onResolveSpec={onResolveSpec}
          />
        ))}
        {resolvedRows}
        {resolvedToggle}
      </div>
    );
  }
  if (resolved.length > 0) {
    return (
      <div className="overview-card">
        <div className="overview-allclear">
          {allClear.length > 0 ? (
            <>
              ✓ {allClear.join(" · ")} <span className="overview-digest-resolved-suffix">· {specItemCount(resolved.length)}</span>
            </>
          ) : (
            <span className="overview-digest-resolved-suffix">{specItemCount(resolved.length)}</span>
          )}
        </div>
        {resolvedRows}
        {resolvedToggle}
      </div>
    );
  }
  if (allClear.length > 0) {
    return <div className="overview-allclear">✓ {allClear.join(" · ")}</div>;
  }
  return null;
}
