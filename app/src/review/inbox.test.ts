// Inbox actions (issue #238 phase 4) against a fake ctx: which existing
// handlers each action calls, and in what order. Run with `bun test`.
import { describe, expect, test } from "bun:test";
import { createInbox } from "./inbox";
import { buildFindings, type Finding } from "./findings";
import type { FileDiff, PrChecksStatus, ReviewManifest, Tab } from "../types";

function fakeCtx(tab: Partial<Tab>) {
  const calls: string[] = [];
  const updates: Array<(t: Tab) => Tab> = [];
  const fullTab = {
    id: "1",
    lens: "files",
    checkedFindings: new Map(),
    resolvedSpecKeys: new Set<string>(),
    specResolutions: new Map(),
    ...tab,
  } as Tab;
  const ctx = {
    activeTabId: "1",
    tabsRef: { current: [fullTab] },
    addToast: (_type: string, msg: string) => calls.push(`toast:${msg.slice(0, 20)}`),
    diffViewerRef: { current: null },
    pendingComposerRef: { current: null as unknown },
    pendingRevealLineRef: { current: null as number | null },
    updateTab: (_id: string, fn: (t: Tab) => Tab) => updates.push(fn),
    unmarkFindingChecked: (k: string) => calls.push(`unmark:${k}`),
    markFindingChecked: (f: Finding) => calls.push(`mark:${f.key}`),
    resolveHighlight: (k: string) => calls.push(`dismiss:${k}`),
    restoreHighlight: (k: string) => calls.push(`restore:${k}`),
    saveResolvedSpecs: (_t: Tab, keys: Set<string>) => calls.push(`saveSpecs:${[...keys].join(",")}`),
    handleChatOpenFile: (p: string, l?: number) => calls.push(`open:${p}:${l}`),
    setSelectedFile: (f: FileDiff) => calls.push(`select:${f.path}`),
    resolveManifestFile: (files: FileDiff[], p: string) => files.find((f) => f.path === p),
  };
  const apply = () => updates.reduce((t, fn) => fn(t), fullTab);
  return { inbox: createInbox(ctx), ctx, calls, apply };
}

const file = { path: "a.ts", head_content: "x\n".repeat(50), diff_hash: "d", highlights: [], classification: "RELEVANT" } as unknown as FileDiff;
const manifest = { files: [file], pr_url: "https://github.com/o/r/pull/1", head_sha: "h" } as unknown as ReviewManifest;
const risk: Finding = { key: "risk:a.ts:5:x", kind: "risk", rank: "check", title: "t", path: "a.ts", startLine: 5, linesHash: "d", state: "checked" };

describe("inbox actions", () => {
  test("Not an issue clears an earlier Looks fine first — one verdict per finding", () => {
    const { inbox, calls } = fakeCtx({ manifest });
    inbox.inboxNotAnIssue(risk, { state: "intentional", reason: "" });
    expect(calls).toEqual(["unmark:risk:a.ts:5:x", "dismiss:risk:a.ts:5:x"]);
  });

  test("Reopen clears both stores", () => {
    const { inbox, calls } = fakeCtx({ manifest });
    inbox.inboxReopen({ ...risk, state: "dismissed" });
    expect(calls).toEqual(["unmark:risk:a.ts:5:x", "restore:risk:a.ts:5:x"]);
  });

  test("Looks fine refuses (and says so) when there's no code to anchor to", () => {
    const { inbox, calls } = fakeCtx({ manifest });
    expect(inbox.inboxLooksFine({ ...risk, linesHash: "" })).toBe(false);
    expect(calls[0]).toStartWith("toast:");
    expect(inbox.inboxLooksFine(risk)).toBe(true);
    expect(calls[1]).toBe("mark:risk:a.ts:5:x");
  });

  test("a CI finding has an anchor, so Looks fine marks it", () => {
    const checks: PrChecksStatus = { overall_state: "failure", check_runs: [{ name: "build", status: "COMPLETED", conclusion: "FAILURE", details_url: null }] };
    const ci = buildFindings(manifest, { checks }).findings[0];
    expect(ci.kind).toBe("ci");
    const { inbox, calls } = fakeCtx({ manifest });
    expect(inbox.inboxLooksFine(ci)).toBe(true);
    expect(calls).toEqual([`mark:${ci.key}`]);
  });

  test("spec actions resolve every requirement in ONE write", () => {
    const { inbox, calls, apply } = fakeCtx({ manifest });
    const spec: Finding = { key: "spec-set:z", kind: "spec", rank: "high", title: "t", itemKeys: ["spec:a", "spec:b"], linesHash: "spec-set:z", state: "open" };
    inbox.inboxNotAnIssue(spec, null);
    expect(calls).toEqual(["saveSpecs:spec:a,spec:b"]);
    expect([...apply().resolvedSpecKeys]).toEqual(["spec:a", "spec:b"]);
  });

  test("a finding in the already-open file queues its reveal instead of calling the viewer", () => {
    const { inbox, ctx, calls, apply } = fakeCtx({ manifest, selectedFile: file, lens: "overview" });
    inbox.selectInboxFinding({ ...risk, state: "open" });
    expect(ctx.pendingRevealLineRef.current).toBe(5);
    expect(calls).toEqual([]); // no handleChatOpenFile round trip
    const t = apply();
    expect(t.inboxSelection).toBe("risk:a.ts:5:x");
    expect(t.lens).toBe("files");
  });

  test("Comment on a finding without a line drafts a PR-level comment", () => {
    const { inbox, calls, apply } = fakeCtx({ manifest, chat: { open: true } as Tab["chat"] });
    const ci: Finding = { key: "ci:1", kind: "ci", rank: "critical", title: "t", items: ["build"], linesHash: "h", state: "open" };
    inbox.inboxComment(ci);
    const t = apply();
    expect(t.commentsOpen).toBe(true);
    expect(t.chat.open).toBe(false); // chat and comments share the dock
    expect(t.prCommentDraft).toBe("CI is failing: build.");
    expect(calls).toEqual([]); // nothing posted, no file opened
  });

  test("Comment on a finding in the open file opens the composer on its lines", () => {
    const { inbox, ctx, calls } = fakeCtx({ manifest, selectedFile: file, lens: "files" });
    const opened: unknown[] = [];
    (ctx.diffViewerRef as { current: unknown }).current = { openComposer: (...a: unknown[]) => (opened.push(a), true) };
    inbox.inboxComment({ ...risk, state: "open", endLine: 7 });
    expect(opened).toEqual([[5, 7, "RIGHT", "t"]]);
    expect(calls).toEqual([]);
  });

  test("Comment says so when the line can't be shown", () => {
    const { inbox, ctx, calls } = fakeCtx({ manifest, selectedFile: file, lens: "files" });
    (ctx.diffViewerRef as { current: unknown }).current = { openComposer: () => false };
    inbox.inboxComment({ ...risk, state: "open" });
    expect(calls[0]).toStartWith("toast:");
  });

  test("Comment on a finding in another file defers the composer until its diff mounts", () => {
    const { inbox, ctx, calls } = fakeCtx({ manifest, selectedFile: null });
    inbox.inboxComment({ ...risk, state: "open" });
    expect(ctx.pendingComposerRef.current).toEqual({ startLine: 5, endLine: 5, side: "RIGHT", initialBody: "t" });
    expect(calls).toEqual(["select:a.ts"]);
  });

  test("a finding in another file opens it through handleChatOpenFile", () => {
    const { inbox, calls, apply } = fakeCtx({ manifest, selectedFile: null });
    inbox.selectInboxFinding({ ...risk, state: "open" });
    expect(calls).toEqual(["open:a.ts:5"]);
    expect(apply().inboxSelectionPath).toBe("a.ts");
  });
});
