// buildFindings (issue #238 phase 3): the merge / dedupe / rank / state rules
// behind the inbox's single findings list. Run with `bun test` from app/.
import { describe, expect, test } from "bun:test";
import { buildFindings, findingCommentBody, firstSentence, riskKey, MERGE_WINDOW } from "./findings";
import { specResolveKey } from "../components/digest";
import { highlightKey } from "../utils";
import type { FileDiff, Highlight, PrChecksStatus, ReviewManifest, ReviewThread, TopRisk } from "../types";

function file(path: string, highlights: Highlight[] = [], extra: Partial<FileDiff> = {}): FileDiff {
  return {
    path,
    highlights,
    classification: "RELEVANT",
    diff_hash: `dh-${path}`,
    head_content: Array.from({ length: 200 }, (_, i) => `${path} line ${i + 1}`).join("\n"),
    ...extra,
  } as FileDiff;
}

function manifest(files: FileDiff[], extra: Partial<ReviewManifest> = {}): ReviewManifest {
  return { files, pr_url: "https://github.com/o/r/pull/1", ...extra } as ReviewManifest;
}

function hl(start: number, end: number, severity: Highlight["severity"], category?: Highlight["category"], comment = `Note at ${start}. More words.`): Highlight {
  return { start_line: start, end_line: end, severity, category, comment };
}

const risk = (path: string, line: number | null, title = "Risky change"): TopRisk => ({ title, detail: "Why it's risky.", path, start_line: line });

describe("buildFindings — sources and ranking", () => {
  test("info notes stay in the diff: counted per file, not findings", () => {
    const m = manifest([file("a.ts", [hl(1, 2, "info", "observation"), hl(5, 6, "info")]), file("b.ts", [hl(3, 3, "warning", "bug")])]);
    const { findings, infoCountByPath } = buildFindings(m);
    expect(findings.map((f) => f.path)).toEqual(["b.ts"]);
    expect(infoCountByPath.get("a.ts")).toBe(2);
    expect(infoCountByPath.has("b.ts")).toBe(false);
  });

  test("an info note with an actionable category is a low finding, not hidden", () => {
    const h: Highlight = { ...hl(446, 446, "info", "test_gap"), fix: "Add a fetch-level test." };
    const { findings, infoCountByPath } = buildFindings(manifest([file("fetch.rs", [h])]));
    expect(findings).toHaveLength(1);
    expect(findings[0]).toMatchObject({ kind: "test_gap", rank: "low", fix: "Add a fetch-level test." });
    expect(infoCountByPath.size).toBe(0);
  });

  test("ranks critical > high defect > risk check > test gap > simplification", () => {
    const m = manifest(
      [file("a.ts", [
        hl(100, 101, "warning", "simplification"),
        hl(80, 81, "warning", "test_gap"),
        hl(60, 61, "warning", "bug"),
        hl(40, 41, "critical", "behavior"),
      ])],
      { triage: { top_risks: [risk("z.ts", null)], review_order: [] } } as Partial<ReviewManifest>,
    );
    const { findings } = buildFindings(m);
    expect(findings.map((f) => [f.kind, f.rank])).toEqual([
      ["behavior", "critical"],
      ["bug", "high"],
      ["risk", "check"],
      ["test_gap", "medium"],
      ["simplification", "low"],
    ]);
  });

  test("uncategorized warnings (older manifests) rank as high notes", () => {
    const { findings } = buildFindings(manifest([file("a.ts", [hl(1, 1, "warning")])]));
    expect(findings[0]).toMatchObject({ kind: "note", rank: "high" });
  });

  test("triage review order breaks ties within a rank, then line", () => {
    const m = manifest(
      [file("a.ts", [hl(50, 50, "warning", "bug"), hl(10, 10, "warning", "bug")]), file("b.ts", [hl(1, 1, "warning", "bug")])],
      { triage: { top_risks: [], review_order: [{ path: "b.ts", rationale: "" }, { path: "a.ts", rationale: "" }] } } as Partial<ReviewManifest>,
    );
    expect(buildFindings(m).findings.map((f) => `${f.path}:${f.startLine}`)).toEqual(["b.ts:1", "a.ts:10", "a.ts:50"]);
  });

  test("headline is the note's first sentence", () => {
    expect(firstSentence("The guard was removed. So anything goes.")).toBe("The guard was removed.");
    expect(firstSentence("No terminal punctuation here")).toBe("No terminal punctuation here");
    expect(firstSentence("x".repeat(200)).length).toBe(140);
  });
});

describe("buildFindings — risk/highlight dedupe", () => {
  test(`a risk within ±${MERGE_WINDOW} lines of a highlight merges into it`, () => {
    const h: Highlight = { ...hl(100, 104, "warning", "test_gap"), scenario: "It bites.", fix: "Add a test." };
    const m = manifest([file("a.ts", [h])], { triage: { top_risks: [risk("a.ts", 110, "Scope widened")], review_order: [] } } as Partial<ReviewManifest>);
    const { findings } = buildFindings(m);
    expect(findings).toHaveLength(1);
    expect(findings[0]).toMatchObject({
      key: highlightKey("a.ts", h),
      kind: "test_gap",
      rank: "check", // stronger of test_gap(medium) and risk(check)
      title: "Scope widened",
      detail: h.comment,
      riskDetail: "Why it's risky.",
      scenario: "It bites.",
      fix: "Add a test.",
    });
  });

  test("keeps the highlight's rank when it outranks the risk", () => {
    const m = manifest([file("a.ts", [hl(10, 10, "critical", "bug")])], { triage: { top_risks: [risk("a.ts", 12)], review_order: [] } } as Partial<ReviewManifest>);
    expect(buildFindings(m).findings[0].rank).toBe("critical");
  });

  test("no merge beyond the window, across files, or without an anchor line", () => {
    const m = manifest(
      [file("a.ts", [hl(100, 104, "warning", "bug")]), file("b.ts", [hl(1, 1, "warning", "bug")])],
      { triage: { top_risks: [risk("a.ts", 104 + MERGE_WINDOW + 1), risk("b.ts", null), risk("c.ts", 1)], review_order: [] } } as Partial<ReviewManifest>,
    );
    const { findings } = buildFindings(m);
    expect(findings.filter((f) => f.kind === "risk")).toHaveLength(3);
    expect(findings.filter((f) => f.kind === "bug")).toHaveLength(2);
  });

  test("each highlight absorbs at most one risk; the more urgent highlight wins", () => {
    const m = manifest(
      [file("a.ts", [hl(10, 10, "warning", "simplification"), hl(14, 14, "critical", "bug")])],
      { triage: { top_risks: [risk("a.ts", 12, "First"), risk("a.ts", 12, "Second")], review_order: [] } } as Partial<ReviewManifest>,
    );
    const { findings } = buildFindings(m);
    const critical = findings.find((f) => f.kind === "bug")!;
    const simp = findings.find((f) => f.kind === "simplification")!;
    expect(critical.title).toBe("First");
    expect(simp.title).toBe("Second");
    expect(findings.filter((f) => f.kind === "risk")).toHaveLength(0);
  });

  test("pairs each risk with the note it points at, not the most urgent nearby one", () => {
    // Greedy most-urgent-first would give "At 18" to the critical note at 10.
    const m = manifest(
      [file("a.ts", [hl(10, 10, "critical", "bug"), hl(18, 18, "warning", "behavior")])],
      { triage: { top_risks: [risk("a.ts", 18, "At 18"), risk("a.ts", 10, "At 10")], review_order: [] } } as Partial<ReviewManifest>,
    );
    const byLine = new Map(buildFindings(m).findings.map((f) => [f.startLine, f.title]));
    expect(byLine.get(10)).toBe("At 10");
    expect(byLine.get(18)).toBe("At 18");
  });

  test("duplicate risks get distinct keys", () => {
    const m = manifest([], { triage: { top_risks: [risk("x.ts", 5, "Same"), risk("x.ts", 5, "Same")], review_order: [] } } as Partial<ReviewManifest>);
    const keys = buildFindings(m).findings.map((f) => f.key);
    expect(new Set(keys).size).toBe(2);
  });

  test("info highlights never absorb a risk", () => {
    const m = manifest([file("a.ts", [hl(10, 10, "info")])], { triage: { top_risks: [risk("a.ts", 10)], review_order: [] } } as Partial<ReviewManifest>);
    expect(buildFindings(m).findings.map((f) => f.kind)).toEqual(["risk"]);
  });
});

describe("buildFindings — spec and CI aggregates", () => {
  const coverage = (statuses: string[]) => ({
    requirements_coverage: {
      requirements: statuses.map((status, i) => ({ text: `Requirement ${i}`, status, tests: [] })),
      orphan_tests: [],
    },
  }) as Partial<ReviewManifest>;

  test("one spec finding for unaddressed partial/uncovered requirements", () => {
    const { findings } = buildFindings(manifest([], coverage(["covered", "partial", "uncovered", "untestable"])));
    expect(findings).toHaveLength(1);
    expect(findings[0]).toMatchObject({
      kind: "spec",
      rank: "high",
      items: ["Requirement 1", "Requirement 2"],
      itemKeys: [specResolveKey("Requirement 1"), specResolveKey("Requirement 2")],
    });
  });

  test("partial-only is medium; addressed requirements drop out; all addressed → none", () => {
    const m = manifest([], coverage(["partial", "partial"]));
    expect(buildFindings(m).findings[0].rank).toBe("medium");
    const one = buildFindings(m, { resolvedSpecKeys: new Set([specResolveKey("Requirement 0")]) }).findings;
    expect(one[0].items).toEqual(["Requirement 1"]);
    const all = new Set([specResolveKey("Requirement 0"), specResolveKey("Requirement 1")]);
    expect(buildFindings(m, { resolvedSpecKeys: all }).findings).toHaveLength(0);
  });

  test("failing CI leads the list even against critical notes in triaged files", () => {
    const m = manifest([file("a.ts", [hl(1, 1, "critical", "bug")])], { triage: { top_risks: [], review_order: [{ path: "a.ts", rationale: "" }] } } as Partial<ReviewManifest>);
    const checks: PrChecksStatus = { overall_state: "failure", check_runs: [{ name: "build", status: "COMPLETED", conclusion: "FAILURE", details_url: null }] };
    expect(buildFindings(m, { checks }).findings.map((f) => f.kind)).toEqual(["ci", "bug"]);
  });

  test("a CI mark lapses on a new head even if the same checks fail", () => {
    const checks: PrChecksStatus = { overall_state: "failure", check_runs: [{ name: "build", status: "COMPLETED", conclusion: "FAILURE", details_url: null }] };
    const f = buildFindings(manifest([], { head_sha: "aaa" }), { checks }).findings[0];
    const checked = new Map([[f.key, { lines_hash: f.linesHash }]]);
    expect(buildFindings(manifest([], { head_sha: "aaa" }), { checks, checked }).findings[0].state).toBe("checked");
    expect(buildFindings(manifest([], { head_sha: "bbb" }), { checks, checked }).findings[0].state).toBe("open");
  });

  test("one critical CI finding only while checks fail", () => {
    const checks = (conclusions: (string | null)[]): PrChecksStatus => ({
      overall_state: "failure",
      check_runs: conclusions.map((conclusion, i) => ({ name: `job${i}`, status: "COMPLETED", conclusion, details_url: null })),
    });
    const failing = buildFindings(manifest([]), { checks: checks(["SUCCESS", "FAILURE", "TIMED_OUT"]) }).findings;
    expect(failing).toHaveLength(1);
    expect(failing[0]).toMatchObject({ kind: "ci", rank: "critical", items: ["job1", "job2"] });
    expect(buildFindings(manifest([]), { checks: checks(["SUCCESS", null]) }).findings).toHaveLength(0);
  });
});

describe("findingCommentBody", () => {
  test("a note drafts like the diff's own Comment…: comment, scenario, fix", () => {
    const h: Highlight = { ...hl(1, 1, "warning", "bug", "Guard removed."), scenario: "Admins bypass it.", fix: "Restore the guard." };
    const f = buildFindings(manifest([file("a.ts", [h])])).findings[0];
    expect(findingCommentBody(f)).toBe("Guard removed.\n\nAdmins bypass it.\n\nSuggested fix: Restore the guard.");
  });

  test("a merged risk still drafts from the note, not the headline", () => {
    const m = manifest([file("a.ts", [hl(10, 10, "warning", "bug", "The note.")])], { triage: { top_risks: [risk("a.ts", 10, "Headline")], review_order: [] } } as Partial<ReviewManifest>);
    expect(findingCommentBody(buildFindings(m).findings[0])).toBe("The note.");
  });
});

describe("buildFindings — state", () => {
  const h = hl(10, 12, "warning", "bug");
  const base = () => manifest([file("a.ts", [h])]);

  test("dismissed keys read as dismissed (and win over checked)", () => {
    const f = buildFindings(base()).findings[0];
    const checked = new Map([[f.key, { lines_hash: f.linesHash }]]);
    expect(buildFindings(base(), { dismissed: new Set([f.key]), checked }).findings[0].state).toBe("dismissed");
  });

  test("Looks fine holds while the lines are unchanged and lapses when they change", () => {
    const f = buildFindings(base()).findings[0];
    const checked = new Map([[f.key, { lines_hash: f.linesHash }]]);
    expect(buildFindings(base(), { checked }).findings[0].state).toBe("checked");

    // A push edits line 11 (inside the finding) → reopens.
    const lines = base().files[0].head_content!.split("\n");
    lines[10] = "changed";
    const edited = manifest([file("a.ts", [h], { head_content: lines.join("\n") })]);
    expect(buildFindings(edited, { checked }).findings[0].state).toBe("open");

    // A push elsewhere in the file (line 50) → still checked.
    const other = base().files[0].head_content!.split("\n");
    other[49] = "changed";
    const elsewhere = manifest([file("a.ts", [h], { head_content: other.join("\n") })]);
    expect(buildFindings(elsewhere, { checked }).findings[0].state).toBe("checked");
  });

  test("a risk-only finding's mark lapses when the file's diff changes", () => {
    const r = risk("a.ts", 5);
    const m = (dh: string) => manifest([file("a.ts", [], { diff_hash: dh })], { triage: { top_risks: [r], review_order: [] } } as Partial<ReviewManifest>);
    const checked = new Map([[riskKey(r), { lines_hash: "dh-1" }]]);
    expect(buildFindings(m("dh-1"), { checked }).findings[0].state).toBe("checked");
    expect(buildFindings(m("dh-2"), { checked }).findings[0].state).toBe("open");
  });

  test("your review thread on a finding's lines marks it commented", () => {
    const thread = (path: string, line: number, login: string) =>
      ({ id: "t", path, line, is_resolved: false, is_outdated: false, original_line: line, diff_hunk: "", comments: [{ author: { login } }] }) as unknown as ReviewThread;
    const at = (threads: ReviewThread[], viewerLogin?: string) => buildFindings(base(), { threads, viewerLogin }).findings[0].state;
    expect(at([thread("a.ts", 11, "me")], "me")).toBe("commented");
    expect(at([thread("a.ts", 11, "someone")], "me")).toBe("open"); // not yours
    expect(at([thread("a.ts", 30, "me")], "me")).toBe("open"); // outside the lines
    expect(at([thread("b.ts", 11, "me")], "me")).toBe("open"); // other file
    const f = buildFindings(base()).findings[0];
    const checked = new Map([[f.key, { lines_hash: f.linesHash }]]);
    expect(buildFindings(base(), { threads: [thread("a.ts", 11, "me")], viewerLogin: "me", checked }).findings[0].state).toBe("checked");
  });

  test("an empty stored hash never counts as checked", () => {
    const f = buildFindings(base()).findings[0];
    const checked = new Map([[f.key, { lines_hash: "" }]]);
    expect(buildFindings(base(), { checked }).findings[0].state).toBe("open");
  });
});
