// The unified findings list behind the inbox layout (issue #238, phase 3).
//
// Today the AI's output is split across surfaces that never meet: triage top
// risks on the Overview, inline highlights in the diff, a requirements card,
// and CI in four places. buildFindings folds them into ONE ranked list — the
// thing the reviewer works through — while info-level observations stay in
// the diff (returned as per-file counts, not findings).
//
// Pure and React-free so the inbox, tests, and anything else can share it.

import { hashString, highlightKey, isFailingCheck } from "../utils";
import { specResolveKey } from "../components/digest";
import type { CheckedFindingEntry, FileDiff, Highlight, PrChecksStatus, ReviewManifest, TopRisk } from "../types";

/** What produced a finding. Highlight categories map 1:1; `note` is a
 * highlight from a manifest cached before categories existed. */
export type FindingKind =
  | "ci"
  | "bug"
  | "behavior"
  | "risk"
  | "test_gap"
  | "simplification"
  | "observation"
  | "note"
  | "spec";

/** Ranking bucket, most urgent first. `check` is a triage risk: a place the
 * reviewer must look at, not a claimed defect — it sits between high-severity
 * defects and medium ones. */
export type FindingRank = "critical" | "high" | "check" | "medium" | "low";

export type FindingState = "open" | "checked" | "dismissed";

export interface Finding {
  /** Stable identity for "Looks fine" / "Not an issue" persistence. */
  key: string;
  kind: FindingKind;
  rank: FindingRank;
  /** One-line headline. */
  title: string;
  /** The full note text (a highlight's comment, or a risk's reason). */
  detail?: string;
  /** Why triage flagged this spot, when a risk merged into a highlight. */
  riskDetail?: string;
  scenario?: string;
  fix?: string;
  path?: string;
  startLine?: number;
  endLine?: number;
  /** Spec/CI findings: the individual items behind the aggregate. */
  items?: string[];
  /** Spec finding: specResolveKey per item. Acting on the aggregate should
   * resolve these (the per-requirement store) rather than key the aggregate,
   * whose key changes whenever the set does. */
  itemKeys?: string[];
  /** Hash of the code the finding sits on; a stored "Looks fine" only holds
   * while this still matches (see isChecked). "" = nothing to anchor on. */
  linesHash: string;
  state: FindingState;
}

export interface FindingsInput {
  /** Dismissed keys ("Not an issue") — the dismissed-highlights store. */
  dismissed?: Set<string>;
  /** "Looks fine" marks — the checked-findings store. */
  checked?: Map<string, CheckedFindingEntry>;
  checks?: PrChecksStatus | null;
  /** Requirements the user already marked addressed (specResolveKey keys). */
  resolvedSpecKeys?: Set<string>;
}

export interface FindingsResult {
  findings: Finding[];
  /** Info-level observations per file — they stay inline in the diff. */
  infoCountByPath: Map<string, number>;
}

/** Risk anchor within this many lines of a highlight's range → same finding. */
export const MERGE_WINDOW = 10;

const RANK_ORDER: Record<FindingRank, number> = { critical: 0, high: 1, check: 2, medium: 3, low: 4 };

export function compareRank(a: FindingRank, b: FindingRank): number {
  return RANK_ORDER[a] - RANK_ORDER[b];
}

function highlightKind(h: Highlight): FindingKind {
  return h.category ?? "note";
}

/** null = not a top-level finding: info-level observations (and
 * uncategorized info from older manifests) stay inline in the diff. An info
 * note with an actionable category — e.g. a test gap with a fix — is still
 * something to act on, so it's a low-rank finding rather than hidden. */
function highlightRank(h: Highlight): FindingRank | null {
  if (h.severity === "info") return h.category && h.category !== "observation" ? "low" : null;
  if (h.severity === "critical") return "critical";
  switch (h.category) {
    case "test_gap":
      return "medium";
    case "simplification":
    case "observation":
      return "low";
    default:
      return "high"; // bug, behavior, or uncategorized (older manifests)
  }
}

/** First sentence of a note, capped — the list headline. */
export function firstSentence(text: string, max = 140): string {
  const t = text.trim();
  const m = t.match(/^(.+?[.!?])(\s|$)/s);
  const s = (m ? m[1] : t).replace(/\s+/g, " ");
  return s.length > max ? s.slice(0, max - 1).trimEnd() + "…" : s;
}

export function riskKey(r: TopRisk): string {
  return `risk:${r.path}:${r.start_line ?? ""}:${hashString(r.title)}`;
}

/** Hash of the head lines [start, end] (1-based, inclusive), falling back to
 * the file's diff hash when head contents weren't fetched. */
function lineRangeHash(file: FileDiff | undefined, start: number, end: number): string {
  if (!file) return "";
  if (file.head_content) {
    const lines = file.head_content.split("\n").slice(Math.max(0, start - 1), Math.max(start, end));
    return hashString(lines.join("\n"));
  }
  return file.diff_hash ?? "";
}

function isChecked(checked: Map<string, CheckedFindingEntry> | undefined, key: string, linesHash: string): boolean {
  const entry = checked?.get(key);
  return !!entry && linesHash !== "" && entry.lines_hash === linesHash;
}

export function buildFindings(manifest: ReviewManifest, input: FindingsInput = {}): FindingsResult {
  const { dismissed, checked, checks, resolvedSpecKeys } = input;
  const byPath = new Map(manifest.files.map((f) => [f.path, f]));
  const infoCountByPath = new Map<string, number>();
  const findings: Omit<Finding, "state">[] = [];

  // ── Highlights (and the risks that land on them) ──
  interface Pending { path: string; h: Highlight; rank: FindingRank; merged?: TopRisk }
  const pending: Pending[] = [];
  for (const file of manifest.files) {
    for (const h of file.highlights) {
      const rank = highlightRank(h);
      if (rank === null) {
        infoCountByPath.set(file.path, (infoCountByPath.get(file.path) ?? 0) + 1);
        continue;
      }
      pending.push({ path: file.path, h, rank });
    }
  }

  // Pair risks with highlights globally, nearest first (then most urgent),
  // so a risk lands on the note it actually points at — greedy per-risk
  // matching could hand two nearby notes each other's headline.
  const risks = manifest.triage?.top_risks ?? [];
  const pairs: { ri: number; p: Pending; dist: number }[] = [];
  risks.forEach((risk, ri) => {
    const line = risk.start_line;
    if (line == null) return;
    for (const p of pending) {
      if (p.path !== risk.path) continue;
      if (line < p.h.start_line - MERGE_WINDOW || line > p.h.end_line + MERGE_WINDOW) continue;
      const dist = line < p.h.start_line ? p.h.start_line - line : line > p.h.end_line ? line - p.h.end_line : 0;
      pairs.push({ ri, p, dist });
    }
  });
  pairs.sort((a, b) => a.dist - b.dist || compareRank(a.p.rank, b.p.rank) || a.ri - b.ri);
  const mergedRisk = new Set<number>();
  for (const { ri, p } of pairs) {
    if (mergedRisk.has(ri) || p.merged) continue;
    p.merged = risks[ri];
    mergedRisk.add(ri);
  }
  const unmergedRisks = risks.filter((_, ri) => !mergedRisk.has(ri));

  for (const p of pending) {
    const { h, merged } = p;
    findings.push({
      key: highlightKey(p.path, h),
      kind: highlightKind(h),
      rank: merged && compareRank("check", p.rank) < 0 ? "check" : p.rank,
      title: merged ? merged.title : firstSentence(h.comment),
      detail: h.comment,
      riskDetail: merged?.detail,
      scenario: h.scenario || undefined,
      fix: h.fix || undefined,
      path: p.path,
      startLine: h.start_line,
      endLine: h.end_line,
      linesHash: lineRangeHash(byPath.get(p.path), h.start_line, h.end_line),
    });
  }

  for (const r of unmergedRisks) {
    findings.push({
      key: riskKey(r),
      kind: "risk",
      rank: "check",
      title: r.title,
      detail: r.detail,
      path: r.path,
      startLine: r.start_line ?? undefined,
      // A risk has only an anchor line, not a range — "its code" is the file's diff.
      linesHash: byPath.get(r.path)?.diff_hash ?? "",
    });
  }

  // ── Spec: one aggregate finding for unaddressed partial/uncovered requirements ──
  const unaddressed = (manifest.requirements_coverage?.requirements ?? []).filter(
    (req) =>
      (req.status === "uncovered" || req.status === "partial") &&
      !resolvedSpecKeys?.has(specResolveKey(req.text)),
  );
  if (unaddressed.length > 0) {
    const itemKeys = unaddressed.map((r) => specResolveKey(r.text));
    const key = `spec-set:${hashString([...itemKeys].sort().join("|"))}`;
    findings.push({
      key,
      kind: "spec",
      rank: unaddressed.some((r) => r.status === "uncovered") ? "high" : "medium",
      title:
        unaddressed.length === 1
          ? "1 requirement isn't fully covered"
          : `${unaddressed.length} requirements aren't fully covered`,
      items: unaddressed.map((r) => r.text),
      itemKeys,
      // The key already encodes the set; a changed set is a new finding.
      linesHash: key,
    });
  }

  // ── CI: one aggregate finding while any check is failing ──
  const failing = (checks?.check_runs ?? []).filter(isFailingCheck);
  if (failing.length > 0) {
    const names = failing.map((c) => c.name);
    const key = `ci:${hashString([...names].sort().join("|"))}`;
    findings.push({
      key,
      kind: "ci",
      rank: "critical",
      title: failing.length === 1 ? `CI: ${names[0]} is failing` : `${failing.length} CI checks are failing`,
      items: names,
      // Tied to the head commit too: the same checks failing again on a new
      // push is a new failure, so a "Looks fine" must not carry over.
      linesHash: hashString(`${key}@${manifest.head_sha}`),
    });
  }

  // ── Order: rank, then triage review order, then position ──
  const orderIndex = new Map((manifest.triage?.review_order ?? []).map((item, i) => [item.path, i]));
  const pos = (f: Omit<Finding, "state">) => (f.path != null ? orderIndex.get(f.path) ?? Infinity : Infinity);
  findings.sort(
    (a, b) =>
      // Failing CI leads regardless of triage order: it blocks the merge.
      Number(b.kind === "ci") - Number(a.kind === "ci") ||
      compareRank(a.rank, b.rank) ||
      pos(a) - pos(b) ||
      (a.path ?? "").localeCompare(b.path ?? "") ||
      (a.startLine ?? 0) - (b.startLine ?? 0),
  );

  // Keys must be unique (list identity + per-finding state): triage can emit
  // two risks with the same path, line, and title.
  const seen = new Map<string, number>();
  for (const f of findings) {
    const n = (seen.get(f.key) ?? 0) + 1;
    seen.set(f.key, n);
    if (n > 1) f.key = `${f.key}#${n}`;
  }

  return {
    findings: findings.map((f) => ({
      ...f,
      state: dismissed?.has(f.key) ? "dismissed" : isChecked(checked, f.key, f.linesHash) ? "checked" : "open",
    })),
    infoCountByPath,
  };
}
