import { hashString, isFailingCheck } from "../utils";
import type { CheckRunInfo, DigestEntry, ReviewManifest, PrChecksStatus } from "../types";

/** Resolve-keys hash canonicalized requirement text so re-extraction drift
 * (case, whitespace, punctuation, markdown backticks/emphasis) doesn't
 * resurface an already-addressed spec item — live-testing showed the model
 * flip-flops on quoting the body's markdown. Everything but letters, digits,
 * and word boundaries is stripped before hashing. Semantic rewording still
 * changes the key — accepted residual. */
export function specResolveKey(text: string): string {
  const normalized = text
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .trim();
  return `spec:${hashString(normalized)}`;
}

/** Per-source tab/row labels for the attention digest (issue #180 follow-up). */
export const DIGEST_SOURCE_LABEL: Record<DigestEntry["source"], string> = {
  ci: "CI",
  triage: "risk",
  coverage: "spec",
};

/** Counts entries per source, omitting sources with zero entries. */
export function countBySource(entries: DigestEntry[]): Partial<Record<DigestEntry["source"], number>> {
  const counts: Partial<Record<DigestEntry["source"], number>> = {};
  for (const entry of entries) {
    counts[entry.source] = (counts[entry.source] ?? 0) + 1;
  }
  return counts;
}

/** Short, jump-worthy wording per failing conclusion — mirrors the CiChip
 * label conventions in PrOverview.tsx. */
function ciClaim(check: CheckRunInfo): string {
  if (check.conclusion === "TIMED_OUT") return `CI: ${check.name} timed out`;
  if (check.conclusion === "ACTION_REQUIRED") return `CI: ${check.name} needs action`;
  return `CI: ${check.name} failed`;
}

/** Builds the ranked "needs your attention" digest (issue #180) by merging
 * CI failures, triage top-risks, and (issue #179 phase 2) a single aggregate
 * coverage entry into one list. CI entries first, then triage, then
 * coverage, each group in its producing pass's own order — no further
 * sorting. Cancelled/skipped runs are deliberately NOT surfaced here (see PR
 * #177: a neutral rail glyph, not a failure), and pending/in-progress runs
 * aren't attention items yet either.
 *
 * Per-requirement/orphan-test rows live in the RequirementsCard now, not
 * here — this digest gets one summary line pointing at that card
 * (`resolvedSpecKeys` is what "addressed" means for the unaddressed count). */
export function buildDigestEntries(
  manifest: ReviewManifest,
  checks?: PrChecksStatus | null,
  resolvedSpecKeys?: Set<string>
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
  const unaddressed = requirements.filter((req) => {
    if (req.status !== "uncovered" && req.status !== "partial") return false;
    return !resolvedSpecKeys?.has(specResolveKey(req.text));
  });
  if (unaddressed.length > 0) {
    const anyUncovered = unaddressed.some((r) => r.status === "uncovered");
    entries.push({
      id: "coverage:summary",
      severity: anyUncovered ? "high" : "medium",
      claim:
        unaddressed.length === 1
          ? "1 requirement needs attention"
          : `${unaddressed.length} requirements need attention`,
      source: "coverage",
      jump: { kind: "requirements" },
    });
  }
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
    // empty-means-None invariant holding forever. At least one requirement
    // must actually be covered — all-untestable means nothing was verified,
    // which is not a "covered" claim.
    const allCovered =
      reqs.some((r) => r.status === "covered") &&
      reqs.every((r) => r.status === "covered" || r.status === "untestable");
    if (allCovered) fragments.push("requirements covered");
  }
  return fragments;
}
