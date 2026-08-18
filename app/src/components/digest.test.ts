// Unit tests for the attention digest (issue #180): the pure functions that
// merge CI failures and triage top-risks into one ranked list, and derive
// the all-clear quiet-line fragments when nothing needs attention.
import { describe, expect, test } from "bun:test";
import { buildAllClearSummary, buildDigestEntries, countBySource, DIGEST_SOURCE_LABEL } from "./digest";
import { hashString } from "../utils";
import type {
  CheckRunInfo,
  DigestEntry,
  PrChecksStatus,
  RequirementEntry,
  RequirementsCoverage,
  ReviewManifest,
  TestRef,
  TopRisk,
} from "../types";

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

function requirement(overrides: Partial<RequirementEntry>): RequirementEntry {
  return {
    text: "Users can reset their password",
    status: "uncovered",
    tests: [],
    note: null,
    ...overrides,
  };
}

function testRef(overrides: Partial<TestRef>): TestRef {
  return { path: "tests/reset.test.ts", note: null, ...overrides };
}

function coverage(overrides: Partial<RequirementsCoverage> = {}): RequirementsCoverage {
  return { requirements: [], orphan_tests: [], ...overrides };
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
    expect(entries[0].resolveKey).toBeUndefined();
    expect(entries[1].source).toBe("triage");
    expect(entries[1].resolveKey).toBeUndefined();
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

  test("uncovered requirement produces a high entry with no jump", () => {
    const m = manifest({
      requirements_coverage: coverage({
        requirements: [requirement({ status: "uncovered", text: "Login rate-limits after 5 tries", note: null })],
      }),
    });
    const entries = buildDigestEntries(m);
    expect(entries.length).toBe(1);
    expect(entries[0]).toMatchObject({
      severity: "high",
      claim: "Login rate-limits after 5 tries",
      source: "coverage",
      jump: { kind: "none" },
      resolveKey: `spec:${hashString("Login rate-limits after 5 tries")}`,
    });
  });

  test("partial requirement with tests produces a medium entry jumping to the first test", () => {
    const m = manifest({
      requirements_coverage: coverage({
        requirements: [
          requirement({
            status: "partial",
            text: "Password reset email is sent",
            note: "only the happy path is asserted",
            tests: [testRef({ path: "tests/reset.test.ts" }), testRef({ path: "tests/reset2.test.ts" })],
          }),
        ],
      }),
    });
    const entries = buildDigestEntries(m);
    expect(entries.length).toBe(1);
    expect(entries[0]).toMatchObject({
      severity: "medium",
      claim: "Password reset email is sent",
      detail: "only the happy path is asserted",
      source: "coverage",
      jump: { kind: "file", path: "tests/reset.test.ts", line: null },
      resolveKey: `spec:${hashString("Password reset email is sent")}`,
    });
  });

  test("partial requirement with no tests produces a medium entry with no jump", () => {
    const m = manifest({
      requirements_coverage: coverage({
        requirements: [requirement({ status: "partial", tests: [] })],
      }),
    });
    const entries = buildDigestEntries(m);
    expect(entries[0].jump).toEqual({ kind: "none" });
  });

  test("covered and untestable requirements produce no entries", () => {
    const m = manifest({
      requirements_coverage: coverage({
        requirements: [requirement({ status: "covered" }), requirement({ status: "untestable" })],
      }),
    });
    expect(buildDigestEntries(m)).toEqual([]);
  });

  test("orphan test produces an info entry with a file jump", () => {
    const m = manifest({
      requirements_coverage: coverage({
        orphan_tests: [testRef({ path: "tests/logout.test.ts", note: "tests logout, not login" })],
      }),
    });
    const entries = buildDigestEntries(m);
    expect(entries.length).toBe(1);
    expect(entries[0]).toMatchObject({
      severity: "info",
      claim: "Test without a stated requirement: logout.test.ts",
      detail: "tests logout, not login",
      source: "coverage",
      jump: { kind: "file", path: "tests/logout.test.ts" },
      resolveKey: `orphan:${hashString("tests/logout.test.ts")}`,
    });
  });

  test("resolveKey is stable for identical text regardless of array position", () => {
    const m = manifest({
      requirements_coverage: coverage({
        requirements: [
          requirement({ status: "uncovered", text: "Padding requirement" }),
          requirement({ status: "uncovered", text: "Users can reset their password" }),
        ],
      }),
    });
    const entries = buildDigestEntries(m);
    const first = manifest({
      requirements_coverage: coverage({
        requirements: [requirement({ status: "uncovered", text: "Users can reset their password" })],
      }),
    });
    const soloEntries = buildDigestEntries(first);
    expect(entries[1].resolveKey).toBe(soloEntries[0].resolveKey);
  });

  test("entries are ordered CI, then triage, then coverage", () => {
    const m = manifest({
      triage: { top_risks: [topRisk({})], review_order: [] },
      requirements_coverage: coverage({
        requirements: [requirement({ status: "uncovered" })],
      }),
    });
    const c = checks([checkRun({ name: "test", conclusion: "FAILURE" })]);
    const entries = buildDigestEntries(m, c);
    expect(entries.map((e) => e.source)).toEqual(["ci", "triage", "coverage"]);
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

  test("all requirements covered/untestable adds the coverage fragment after triage", () => {
    const m = manifest({
      triage: { top_risks: [], review_order: [] },
      requirements_coverage: coverage({
        requirements: [requirement({ status: "covered" }), requirement({ status: "untestable" })],
      }),
    });
    expect(buildAllClearSummary(m, null)).toEqual(["no top risks", "requirements covered"]);
  });

  test("coverage absent yields no requirements fragment", () => {
    expect(buildAllClearSummary(manifest({ triage: null }), null)).toEqual([]);
  });

  test("coverage present but empty yields no vacuous fragment", () => {
    const m = manifest({ triage: null, requirements_coverage: coverage() });
    expect(buildAllClearSummary(m, null)).toEqual([]);
  });

  test("coverage present with an uncovered requirement yields no all-clear fragment", () => {
    const m = manifest({
      requirements_coverage: coverage({
        requirements: [requirement({ status: "uncovered" })],
      }),
    });
    expect(buildAllClearSummary(m, null)).toEqual([]);
  });
});

// ── DIGEST_SOURCE_LABEL / countBySource ─────────────────────────────────────

function digestEntry(overrides: Partial<DigestEntry>): DigestEntry {
  return {
    id: "x:0",
    severity: "high",
    claim: "claim",
    source: "triage",
    jump: { kind: "none" },
    ...overrides,
  };
}

describe("DIGEST_SOURCE_LABEL", () => {
  test("maps all three sources", () => {
    expect(DIGEST_SOURCE_LABEL).toEqual({ ci: "CI", triage: "risk", coverage: "spec" });
  });
});

describe("countBySource", () => {
  test("counts entries per source", () => {
    const entries = [
      digestEntry({ id: "ci:0", source: "ci" }),
      digestEntry({ id: "ci:1", source: "ci" }),
      digestEntry({ id: "risk:0", source: "triage" }),
      digestEntry({ id: "req:0", source: "coverage" }),
    ];
    expect(countBySource(entries)).toEqual({ ci: 2, triage: 1, coverage: 1 });
  });

  test("omits sources with no entries", () => {
    const entries = [digestEntry({ id: "ci:0", source: "ci" })];
    expect(countBySource(entries)).toEqual({ ci: 1 });
  });

  test("empty entries yields an empty object", () => {
    expect(countBySource([])).toEqual({});
  });
});
