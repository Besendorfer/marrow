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
  return null;
}

// One interaction model for every row: click toggles the full text open
// (clamped when collapsed), and navigation is an explicit action button
// inside the expanded row — never the row click itself.
function DigestRow({
  entry,
  expanded,
  onToggle,
  onOpenAt,
  onOpenChecks,
}: {
  entry: DigestEntry;
  expanded: boolean;
  onToggle: () => void;
  onOpenAt?: (path: string, line?: number) => void;
  onOpenChecks?: () => void;
}) {
  function jump() {
    if (entry.jump.kind === "file") onOpenAt?.(entry.jump.path, entry.jump.line ?? undefined);
    else if (entry.jump.kind === "checks") onOpenChecks?.();
    else if (entry.jump.kind === "url") onOpenChecks?.(); // MVP: url jumps fall back to the Checks lens.
  }
  const label = jumpLabel(entry);
  return (
    <div className={expanded ? "overview-digest-row expanded" : "overview-digest-row"}>
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
      {expanded && label && (
        <div className="overview-digest-row-actions">
          <button className="overview-digest-row-action" onClick={jump}>
            {label} →
          </button>
        </div>
      )}
    </div>
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
  const [tab, setTab] = useState<"all" | DigestEntry["source"]>("all");
  const [expandedId, setExpandedId] = useState<string | null>(null);
  if (entries.length > 0) {
    const counts = countBySource(entries);
    const sources = TAB_ORDER.filter((source) => counts[source]);
    // Fall back to "all" if the selected tab's source has since emptied out,
    // rather than setState-during-render.
    const effectiveTab = tab !== "all" && !counts[tab] ? "all" : tab;
    const visibleEntries = effectiveTab === "all" ? entries : entries.filter((e) => e.source === effectiveTab);
    return (
      <div className="overview-card">
        <h4>Needs your attention</h4>
        {sources.length >= 2 && (
          <div className="overview-digest-tabs">
            <button
              className={effectiveTab === "all" ? "overview-digest-tab active" : "overview-digest-tab"}
              onClick={() => setTab("all")}
            >
              All ({entries.length})
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
          />
        ))}
      </div>
    );
  }
  if (allClear.length > 0) {
    return <div className="overview-allclear">✓ {allClear.join(" · ")}</div>;
  }
  return null;
}
