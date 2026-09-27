// Finish panel rules (issue #238 phase 5). Run with `bun test` from app/.
import { describe, expect, test } from "bun:test";
import { ciStatus, filesReviewed, pendingComments, submitBlocker } from "./finish";
import type { FileDiff, PrChecksStatus, ReviewManifest, ReviewThread } from "../types";

const thread = (id: string, comments: { body: string; pending?: boolean }[]) =>
  ({ id, path: "a.ts", line: 3, is_resolved: false, comments }) as unknown as ReviewThread;

describe("pendingComments", () => {
  test("lists only comments in your pending review", () => {
    const got = pendingComments([
      thread("t1", [{ body: "posted" }, { body: "queued reply", pending: true }]),
      thread("t2", [{ body: "queued", pending: true }]),
    ]);
    expect(got.map((p) => p.body)).toEqual(["queued reply", "queued"]);
    expect(pendingComments(undefined)).toEqual([]);
  });
});

describe("submitBlocker", () => {
  test("merged PRs only take a comment review", () => {
    expect(submitBlocker("APPROVE", "", 0, true)).toContain("merged");
    expect(submitBlocker("REQUEST_CHANGES", "fix it", 0, true)).toContain("merged");
    expect(submitBlocker("COMMENT", "thanks", 0, true)).toBeNull();
  });

  test("request changes needs a body", () => {
    expect(submitBlocker("REQUEST_CHANGES", "  ", 3, false)).toBe("Say what needs to change.");
    expect(submitBlocker("REQUEST_CHANGES", "Please add a test", 0, false)).toBeNull();
  });

  test("a comment review needs a body or at least one batched comment", () => {
    expect(submitBlocker("COMMENT", "", 0, false)).not.toBeNull();
    expect(submitBlocker("COMMENT", "", 2, false)).toBeNull();
    expect(submitBlocker("COMMENT", "LGTM", 0, false)).toBeNull();
  });

  test("approve can be empty", () => {
    expect(submitBlocker("APPROVE", "", 0, false)).toBeNull();
  });
});

describe("ciStatus", () => {
  const run = (conclusion: string | null, status = "COMPLETED", name = "build") => ({ name, status, conclusion, details_url: null });
  const checks = (...runs: ReturnType<typeof run>[]): PrChecksStatus => ({ overall_state: "x", check_runs: runs });

  test("failing beats running beats passing; nothing reported is its own state", () => {
    expect(ciStatus(checks(run("FAILURE"), run(null, "IN_PROGRESS", "lint")))).toEqual({ tone: "fail", text: "CI: build is failing" });
    expect(ciStatus(checks(run("FAILURE"), run("TIMED_OUT", "COMPLETED", "e2e"))).text).toBe("CI: 2 checks failing");
    expect(ciStatus(checks(run("SUCCESS"), run(null, "IN_PROGRESS")))).toEqual({ tone: "running", text: "CI is still running" });
    expect(ciStatus(checks(run("SUCCESS"))).tone).toBe("ok");
    expect(ciStatus(null).tone).toBe("none");
  });
});

describe("filesReviewed", () => {
  test("counts relevant files only", () => {
    const m = {
      files: [
        { path: "a", classification: "RELEVANT" },
        { path: "b", classification: "RELEVANT" },
        { path: "c", classification: "NOT_RELEVANT" },
      ] as FileDiff[],
    } as ReviewManifest;
    expect(filesReviewed(m, new Set(["a", "c"]))).toEqual({ reviewed: 1, total: 2 });
  });
});
