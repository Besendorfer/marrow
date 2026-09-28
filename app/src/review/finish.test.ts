// Finish panel rules (issue #238 phase 5). Run with `bun test` from app/.
import { describe, expect, test } from "bun:test";
import { attemptSubmit, ciStatus, createFinish, defaultVerb, filesReviewed, isNextCandidate, mergeDraft, pendingComments, recapSummary, submitBlocker } from "./finish";
import { canonicalPrKey, ciChip } from "../utils";
import type { FileDiff, PrChecksStatus, ReviewManifest, ReviewRequestItem, ReviewThread, Tab } from "../types";

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

describe("mergeDraft", () => {
  test("an AI draft never replaces typed text", () => {
    expect(mergeDraft("", "AI text")).toBe("AI text");
    expect(mergeDraft("   ", "AI text")).toBe("AI text");
    expect(mergeDraft("my words", "AI text")).toBe("my words");
  });
});

describe("defaultVerb", () => {
  test("comment on merged PRs, request changes while defects are open, else approve", () => {
    expect(defaultVerb(true, 3)).toBe("COMMENT");
    expect(defaultVerb(false, 2)).toBe("REQUEST_CHANGES");
    expect(defaultVerb(false, 0)).toBe("APPROVE");
  });
});

describe("isNextCandidate", () => {
  const item = (over: Partial<ReviewRequestItem> = {}) =>
    ({ owner: "o", repo: "r", number: 7, draft: false, my_review_status: "pending", ...over }) as ReviewRequestItem;
  test("only PRs still waiting on you, not drafts, not already open", () => {
    const none = new Set<string | null>();
    expect(isNextCandidate(item(), none)).toBe(true);
    expect(isNextCandidate(item({ my_review_status: "dismissed" }), none)).toBe(true);
    expect(isNextCandidate(item({ my_review_status: "approved" }), none)).toBe(false);
    expect(isNextCandidate(item({ my_review_status: "commented" }), none)).toBe(false);
    expect(isNextCandidate(item({ draft: true }), none)).toBe(false);
    expect(isNextCandidate(item(), new Set([canonicalPrKey("https://github.com/o/r/pull/7")]))).toBe(false);
  });
});

describe("ciChip (Overview)", () => {
  const run = (conclusion: string | null, status = "COMPLETED") => ({ name: "b", status, conclusion, details_url: null });
  test("no runs means no chip, even though core reports overall success", () => {
    expect(ciChip({ overall_state: "success", check_runs: [] })).toBeNull();
    expect(ciChip({ overall_state: "success", check_runs: [run("SUCCESS")] })?.label).toBe("CI passing");
    expect(ciChip({ overall_state: "failure", check_runs: [run("FAILURE")] })?.label).toBe("1 CI check failing");
    expect(ciChip({ overall_state: "pending", check_runs: [run(null, "IN_PROGRESS")] })?.label).toBe("CI running");
  });
});

describe("recapSummary", () => {
  const f = (urgency: "fix" | "look", state: "open" | "checked" = "open") => ({ urgency, state });
  test("open defects lead; open looks follow; handled ones don't count", () => {
    expect(recapSummary([f("look"), f("look"), f("look", "checked")])).toBe("Nothing to fix · 2 worth a look still open");
    expect(recapSummary([f("fix"), f("fix", "checked"), f("look")])).toBe("1 to fix · 1 worth a look still open");
    expect(recapSummary([])).toBe("Nothing to fix");
  });
});

describe("attemptSubmit", () => {
  test("success reports done with the pre-counted batched comments", async () => {
    const calls: string[] = [];
    const got = await attemptSubmit(async (e, b) => { calls.push(`${e}:${b}`); }, "APPROVE", "  LGTM  ", 2);
    expect(got).toEqual({ ok: true, done: { event: "APPROVE", posted: 2 } });
    expect(calls).toEqual(["APPROVE:LGTM"]);
  });

  test("a failed submit reports the error instead of throwing, and isn't done", async () => {
    const got = await attemptSubmit(async () => { throw "Can not approve your own pull request"; }, "APPROVE", "", 0);
    expect(got).toEqual({ ok: false, error: "Can not approve your own pull request" });
  });
});

describe("createFinish — per-tab panel state", () => {
  function run(tabs: Partial<Tab>[], activeTabId: string) {
    let state = tabs.map((t) => ({ manifest: {}, ...t }) as Tab);
    const ctx = {
      activeTabId,
      tabsRef: { get current() { return state; } },
      updateTab: (id: string, fn: (t: Tab) => Tab) => { state = state.map((t) => (t.id === id ? fn(t) : t)); },
    };
    return { finish: createFinish(ctx), get: (id: string) => state.find((t) => t.id === id)! };
  }

  test("opening affects only the active tab", () => {
    const { finish, get } = run([{ id: "a" }, { id: "b" }], "a");
    finish.openFinish();
    expect(get("a").finishOpen).toBe(true);
    expect(get("b").finishOpen).toBeFalsy();
  });

  test("closing keeps an unsent draft; after a submit the next open starts fresh", () => {
    const { finish, get } = run([{ id: "a", finishOpen: true }], "a");
    finish.setFinishDraft({ body: "half-written", verb: "COMMENT" });
    finish.closeFinish();
    expect(get("a").finishDraft).toEqual({ body: "half-written", verb: "COMMENT" });
    finish.openFinish();
    finish.setFinishDone({ event: "COMMENT", posted: 1 });
    finish.closeFinish();
    expect(get("a").finishDraft).toBeNull();
    expect(get("a").finishDone).toBeNull();
  });
});
