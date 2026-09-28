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
import type { CheckedFindingEntry, FileDiff, FindingRelation, Highlight, PrChecksStatus, ReviewManifest, ReviewThread, TopRisk } from "../types";

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

/** `commented` = you left a review comment on the finding's lines (derived
 * from GitHub threads, not stored). */
export type FindingState = "open" | "checked" | "commented" | "dismissed";

/** Does this need a fix before merge, or is it worth a look? Only a claimed
 * defect (a critical/high bug or behavior note, or failing CI) is "fix".
 * Triage risks are "look": the triage pass names the riskiest places to
 * review, it never claims they're wrong. Test gaps, uncovered requirements,
 * simplifications, and observations are "look" too. */
export type FindingUrgency = "fix" | "look";

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
  urgency: FindingUrgency;
  /** Hash of the code the finding sits on; a stored "Looks fine" only holds
   * while this still matches (see isChecked). "" = nothing to anchor on. */
  linesHash: string;
  state: FindingState;
  /** Jev (issue #249): the key of the finding this one is grouped under —
   * one root cause, a different action (e.g. the test for a bug). */
  parentKey?: string;
  /** Jev: the same problem reported again, merged into this finding. Each
   * keeps its own key, so a verdict on this finding is applied to them too. */
  duplicates?: Finding[];
}

export interface FindingsInput {
  /** Dismissed keys ("Not an issue") — the dismissed-highlights store. */
  dismissed?: Set<string>;
  /** "Looks fine" marks — the checked-findings store. */
  checked?: Map<string, CheckedFindingEntry>;
  checks?: PrChecksStatus | null;
  /** Requirements the user already marked addressed (specResolveKey keys). */
  resolvedSpecKeys?: Set<string>;
  /** Review threads on the PR; a thread you started on a finding's lines
   * marks it `commented`. */
  threads?: ReviewThread[];
  /** Your GitHub login (thread authorship). Unknown → only just-posted ("you") threads count. */
  viewerLogin?: string | null;
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

/** Did the viewer start a review thread on this finding's lines? A range
 * finding (a note) matches threads inside its lines; an anchor-only finding
 * (a risk) matches within the merge window, like the risk/note pairing. */
function isCommented(f: Omit<Finding, "state" | "urgency">, threads: ReviewThread[] | undefined, viewer: string | null | undefined): boolean {
  if (!threads?.length || f.path == null || f.startLine == null) return false;
  const start = f.endLine != null ? f.startLine : f.startLine - MERGE_WINDOW;
  const end = f.endLine ?? f.startLine + MERGE_WINDOW;
  return threads.some((t) => {
    if (t.path !== f.path || t.line == null || t.line < start || t.line > end) return false;
    const author = t.comments[0]?.author.login;
    // "you" is the optimistic placeholder a just-posted comment renders with.
    // Until your login is known, only that counts — someone else's thread
    // must not read as yours.
    return author === "you" || (!!viewer && author === viewer);
  });
}

function urgencyOf(kind: FindingKind, rank: FindingRank): FindingUrgency {
  if (kind === "ci") return "fix";
  const defect = kind === "bug" || kind === "behavior" || kind === "note";
  return defect && (rank === "critical" || rank === "high") ? "fix" : "look";
}

/** One plain sentence on what the AI is (and isn't) claiming — so a
 * reviewer can tell "this is broken" from "look here" at a glance. */
export function findingClaim(f: Pick<Finding, "kind" | "urgency">): string {
  if (f.kind === "ci") return "CI is failing on this PR.";
  if (f.urgency === "fix") return "The AI thinks this is broken and needs a fix before merge.";
  switch (f.kind) {
    case "risk":
      return "No defect claimed. It's one of the riskiest changes in this PR, so verify it yourself.";
    case "test_gap":
      return "Not a bug: behavior no test checks yet. Worth closing, not blocking.";
    case "spec":
      return "Requirements from the PR description that no test proves yet.";
    case "simplification":
      return "Optional cleanup; nothing is broken.";
    default:
      return "Worth knowing; the AI isn't asking for a change.";
  }
}

/** The review-list selection id for a finding. Spec and CI aggregates get
 * fixed ids: their keys hash the current item set, which changes while you
 * work (resolving a requirement, a check starting to pass), and the
 * selection must not jump away when it does. */
export function selectionIdFor(f: Pick<Finding, "kind" | "key">): string {
  return f.kind === "spec" || f.kind === "ci" ? f.kind : f.key;
}

/** Draft body when the reviewer chooses "Comment" on a finding: the note in
 * the reviewer's own words — same shape as highlightCommentBody (utils.ts) for
 * notes, the risk's reason for risks, and the item list for aggregates. The
 * reviewer edits it before anything posts. */
export function findingCommentBody(f: Finding): string {
  if (f.kind === "spec") return ["These requirements aren't fully covered yet:", ...(f.items ?? []).map((i) => `- ${i}`)].join("\n");
  if (f.kind === "ci") return `CI is failing: ${(f.items ?? []).join(", ")}.`;
  if (f.kind === "risk") return [f.title, f.detail].filter(Boolean).join("\n\n");
  const parts = [f.detail ?? f.title];
  if (f.scenario) parts.push(f.scenario);
  if (f.fix) parts.push(`Suggested fix: ${f.fix}`);
  return parts.join("\n\n");
}

function isChecked(checked: Map<string, CheckedFindingEntry> | undefined, key: string, linesHash: string): boolean {
  const entry = checked?.get(key);
  return !!entry && linesHash !== "" && entry.lines_hash === linesHash;
}

export function buildFindings(manifest: ReviewManifest, input: FindingsInput = {}): FindingsResult {
  const { dismissed, checked, checks, resolvedSpecKeys, threads, viewerLogin } = input;
  const byPath = new Map(manifest.files.map((f) => [f.path, f]));
  const infoCountByPath = new Map<string, number>();
  const findings: Omit<Finding, "state" | "urgency">[] = [];

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
  const pos = (f: Omit<Finding, "state" | "urgency">) => (f.path != null ? orderIndex.get(f.path) ?? Infinity : Infinity);
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

  const stated: Finding[] = findings.map((f) => ({
      ...f,
      urgency: urgencyOf(f.kind, f.rank),
      state: dismissed?.has(f.key)
        ? "dismissed"
        : isChecked(checked, f.key, f.linesHash)
          ? "checked"
          : isCommented(f, threads, viewerLogin)
            ? "commented"
            : ("open" as const),
    }));
  return {
    findings: applyRelations(stated, manifest.finding_relations),
    infoCountByPath,
  };
}

/** Apply Jev's pair calls (issue #249) to the ranked list. The finding that
 * ranks first in a pair is the primary. A "same" partner is folded into the
 * primary's `duplicates` and leaves the list; a "related" one stays in the
 * list, right after its primary, with `parentKey` set. One level only: a
 * pair that would nest deeper attaches to the root instead, and a finding
 * that already heads a group isn't pulled under another. Strongest calls
 * are applied first. */
export function applyRelations(findings: Finding[], relations: FindingRelation[] | undefined): Finding[] {
  if (!relations?.length) return findings;
  const index = new Map(findings.map((f, i) => [f.key, i]));
  const parentOf = new Map<string, { key: string; relation: "same" | "related" }>();
  const heads = new Set<string>();
  const strength = (r: FindingRelation) => Math.max(r.p_same, r.p_related);
  for (const r of [...relations].sort((x, y) => strength(y) - strength(x))) {
    const ka = highlightKey(r.a.path, r.a);
    const kb = highlightKey(r.b.path, r.b);
    const ia = index.get(ka);
    const ib = index.get(kb);
    if (ia == null || ib == null || ia === ib) continue;
    let head = ia < ib ? ka : kb;
    const child = ia < ib ? kb : ka;
    head = parentOf.get(head)?.key ?? head;
    if (head === child || parentOf.has(child) || heads.has(child)) continue;
    parentOf.set(child, { key: head, relation: r.relation });
    heads.add(head);
  }
  if (parentOf.size === 0) return findings;
  const dups = new Map<string, Finding[]>();
  const kids = new Map<string, Finding[]>();
  for (const f of findings) {
    const p = parentOf.get(f.key);
    if (!p) continue;
    const bucket = p.relation === "same" ? dups : kids;
    bucket.set(p.key, [...(bucket.get(p.key) ?? []), p.relation === "same" ? f : { ...f, parentKey: p.key }]);
  }
  const out: Finding[] = [];
  for (const f of findings) {
    if (parentOf.has(f.key)) continue;
    out.push(dups.has(f.key) ? { ...f, duplicates: dups.get(f.key) } : f);
    out.push(...(kids.get(f.key) ?? []));
  }
  return out;
}

