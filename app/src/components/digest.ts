import { isFailingCheck } from "../utils";
import type { CheckRunInfo, DigestEntry, ReviewManifest, PrChecksStatus } from "../types";

/** Short, jump-worthy wording per failing conclusion — mirrors the CiChip
 * label conventions in PrOverview.tsx. */
function ciClaim(check: CheckRunInfo): string {
  if (check.conclusion === "TIMED_OUT") return `CI: ${check.name} timed out`;
  if (check.conclusion === "ACTION_REQUIRED") return `CI: ${check.name} needs action`;
  return `CI: ${check.name} failed`;
}

/** Builds the ranked "needs your attention" digest (issue #180) by merging
 * CI failures and triage top-risks into one list. CI entries first, then
 * triage, each group in its producing pass's own order — no further sorting.
 * Cancelled/skipped runs are deliberately NOT surfaced here (see PR #177: a
 * neutral rail glyph, not a failure), and pending/in-progress runs aren't
 * attention items yet either. */
export function buildDigestEntries(
  manifest: ReviewManifest,
  checks?: PrChecksStatus | null
): DigestEntry[] {
  const entries: DigestEntry[] = [];
  (checks?.check_runs ?? []).forEach((check, i) => {
    if (check.status !== "COMPLETED") return;
    if (!isFailingCheck(check)) return;
    // Index in the id: check-run names aren't unique (same guard as ChecksLens).
    entries.push({
      id: `ci:${i}:${check.name}`,
      severity: "critical",
      claim: ciClaim(check),
      source: "ci",
      jump: { kind: "checks" },
    });
  });
  const topRisks = manifest.triage?.top_risks ?? [];
  topRisks.forEach((risk, i) => {
    entries.push({
      id: `risk:${i}`,
      severity: "high",
      claim: risk.title,
      detail: risk.detail,
      source: "triage",
      jump: { kind: "file", path: risk.path, line: risk.start_line },
    });
  });
  return entries;
}

/** Quiet-line fragments for the all-clear state — only for signals that
 * actually ran (absence, not empty claims). Returns [] when nothing ran, so
 * the digest renders nothing at all. */
export function buildAllClearSummary(
  manifest: ReviewManifest,
  checks?: PrChecksStatus | null
): string[] {
  const fragments: string[] = [];
  if (checks) {
    const anyFailing = checks.check_runs.some(isFailingCheck);
    const anyPending = checks.check_runs.some((r) => r.status !== "COMPLETED");
    if (!anyFailing) fragments.push(anyPending ? "CI running" : "CI green");
  }
  if (manifest.triage) {
    if (manifest.triage.top_risks.length === 0) fragments.push("no top risks");
  }
  return fragments;
}
