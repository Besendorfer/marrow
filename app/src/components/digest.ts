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
    // Conclusion-only test, same as countFailingChecks (utils.ts): a run
    // only carries a failing conclusion once completed, so no status guard.
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
  const requirements = manifest.requirements_coverage?.requirements ?? [];
  requirements.forEach((req, i) => {
    if (req.status === "uncovered") {
      entries.push({
        id: `req:${i}`,
        severity: "high",
        claim: req.text,
        detail: req.note ?? undefined,
        source: "coverage",
        jump: { kind: "none" },
      });
    } else if (req.status === "partial") {
      const firstTestPath = req.tests[0]?.path;
      entries.push({
        id: `req:${i}`,
        severity: "medium",
        claim: req.text,
        detail: req.note ?? undefined,
        source: "coverage",
        jump: firstTestPath ? { kind: "file", path: firstTestPath, line: null } : { kind: "none" },
      });
    }
  });
  const orphanTests = manifest.requirements_coverage?.orphan_tests ?? [];
  orphanTests.forEach((test, i) => {
    const filename = test.path.split("/").pop() ?? test.path;
    entries.push({
      id: `orphan:${i}`,
      severity: "info",
      claim: `Test without a stated requirement: ${filename}`,
      detail: test.note ?? undefined,
      source: "coverage",
      jump: { kind: "file", path: test.path },
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
  // Empty check_runs means no CI is configured — that's an absent signal,
  // not a green one.
  if (checks && checks.check_runs.length > 0) {
    const anyFailing = checks.check_runs.some(isFailingCheck);
    const anyPending = checks.check_runs.some((r) => r.status !== "COMPLETED");
    if (!anyFailing) fragments.push(anyPending ? "CI running" : "CI green");
  }
  if (manifest.triage) {
    if (manifest.triage.top_risks.length === 0) fragments.push("no top risks");
  }
  if (manifest.requirements_coverage) {
    const reqs = manifest.requirements_coverage.requirements;
    // Guard the vacuous-.every() case locally rather than relying on core's
    // empty-means-None invariant holding forever.
    const allCovered =
      reqs.length > 0 &&
      reqs.every((r) => r.status === "covered" || r.status === "untestable");
    if (allCovered) fragments.push("requirements covered");
  }
  return fragments;
}
