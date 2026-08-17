// Unit tests for the attention digest (issue #180): the pure functions that
// merge CI failures and triage top-risks into one ranked list, and derive
// the all-clear quiet-line fragments when nothing needs attention.
import { describe, expect, test } from "bun:test";
import { buildAllClearSummary, buildDigestEntries } from "./digest";
import type { CheckRunInfo, PrChecksStatus, ReviewManifest, TopRisk } from "../types";

function checkRun(overrides: Partial<CheckRunInfo>): CheckRunInfo {
  return {
    name: "build",
    status: "COMPLETED",
    conclusion: "SUCCESS",
    details_url: null,
    ...overrides,
  };
}

function checks(runs: CheckRunInfo[]): PrChecksStatus {
  return { overall_state: "failure", check_runs: runs };
}

function topRisk(overrides: Partial<TopRisk>): TopRisk {
  return {
    title: "Admin-only check removed on /users",
    detail: "Bypasses the role guard",
    path: "src/routes/users.ts",
    start_line: 42,
    ...overrides,
  };
}

function manifest(overrides: Partial<ReviewManifest> = {}): ReviewManifest {
  return {
    pr_title: "Test PR",
    pr_url: "https://github.com/x/y/pull/1",
    pr_number: 1,
    base_ref: "main",
    head_ref: "feat",
    base_sha: "aaa",
    head_sha: "bbb",
    author: "octocat",
    draft: false,
    summary: "",
    change_groups: [],
    triage: null,
    body: "",
    commits: [],
    files: [],
    ...overrides,
  };
}

// ── buildDigestEntries ───────────────────────────────────────────────────────

describe("buildDigestEntries", () => {
  test("CI failure produces a critical entry before triage entries", () => {
    const m = manifest({ triage: { top_risks: [topRisk({})], review_order: [] } });
    const c = checks([checkRun({ name: "test", conclusion: "FAILURE" })]);
    const entries = buildDigestEntries(m, c);
    expect(entries.length).toBe(2);
    expect(entries[0].source).toBe("ci");
    expect(entries[0].severity).toBe("critical");
    expect(entries[1].source).toBe("triage");
  });

  test("cancelled, skipped, and pending runs produce no entries", () => {
    const c = checks([
      checkRun({ name: "a", conclusion: "CANCELLED" }),
      checkRun({ name: "b", conclusion: "SKIPPED" }),
      checkRun({ name: "c", status: "IN_PROGRESS", conclusion: null }),
    ]);
    expect(buildDigestEntries(manifest(), c)).toEqual([]);
  });

  test("triage order is preserved", () => {
    const m = manifest({
      triage: {
        top_risks: [
          topRisk({ title: "First" }),
          topRisk({ title: "Second" }),
          topRisk({ title: "Third" }),
        ],
        review_order: [],
      },
    });
    const entries = buildDigestEntries(m);
    expect(entries.map((e) => e.claim)).toEqual(["First", "Second", "Third"]);
  });
});

// ── buildAllClearSummary ─────────────────────────────────────────────────────

describe("buildAllClearSummary", () => {
  test("both signals healthy", () => {
    const m = manifest({ triage: { top_risks: [], review_order: [] } });
    const c = checks([checkRun({})]);
    expect(buildAllClearSummary(m, c)).toEqual(["CI green", "no top risks"]);
  });

  test("no checks data yields only the triage fragment", () => {
    const m = manifest({ triage: { top_risks: [], review_order: [] } });
    expect(buildAllClearSummary(m, null)).toEqual(["no top risks"]);
  });

  test("triage null yields only the CI fragment", () => {
    const c = checks([checkRun({})]);
    expect(buildAllClearSummary(manifest({ triage: null }), c)).toEqual(["CI green"]);
  });

  test("nothing ran returns an empty list", () => {
    expect(buildAllClearSummary(manifest({ triage: null }), null)).toEqual([]);
  });

  test("empty check_runs (no CI configured) yields no CI fragment", () => {
    const m = manifest({ triage: { top_risks: [], review_order: [] } });
    expect(buildAllClearSummary(m, checks([]))).toEqual(["no top risks"]);
  });

  test("pending-but-not-failing checks report CI running", () => {
    const c = checks([checkRun({ status: "IN_PROGRESS", conclusion: null })]);
    expect(buildAllClearSummary(manifest({ triage: null }), c)).toEqual(["CI running"]);
  });
});
