// Dismissal persistence (issue #252): per-key writes, in order, and the
// merged on-disk state (including another writer's keys) comes back to the tab.
import { describe, expect, mock, test } from "bun:test";

// A fake of the backend's per-key update over a "disk" the test controls.
const disk = { keys: [] as string[], resolutions: {} as Record<string, { state: string }> };
const calls: string[] = [];
let slowFirst = true;
mock.module("@tauri-apps/api/core", () => ({
  invoke: async (cmd: string, args: { key: string; resolution?: { state: string } | null }) => {
    calls.push(`${cmd}:${args.key}`);
    if (cmd === "dismiss_highlight") {
      if (!disk.keys.includes(args.key)) disk.keys.push(args.key);
      if (args.resolution) disk.resolutions[args.key] = args.resolution;
      else delete disk.resolutions[args.key];
    } else if (cmd === "restore_dismissed_highlight") {
      disk.keys = disk.keys.filter((k) => k !== args.key);
      delete disk.resolutions[args.key];
    }
    // Snapshot now, answer later: the first write's response is slow, so if
    // writes weren't queued the second's newer state would land first and
    // then be rolled back by this stale one.
    const snapshot = { keys: [...disk.keys], resolutions: { ...disk.resolutions } };
    if (slowFirst) {
      slowFirst = false;
      await new Promise((r) => setTimeout(r, 30));
    }
    return snapshot;
  },
}));

const { createProgress, dismissalVersionOf } = await import("./progress");

const history: Set<string>[] = [];

function fakeCtx() {
  let tab = {
    id: "t1",
    manifest: { pr_url: "https://github.com/o/r/pull/7" },
    dismissedHighlights: new Set<string>(),
    noteResolutions: new Map(),
  } as any;
  const tabsRef = { current: [tab] };
  const ctx = {
    activeTabId: "t1",
    tabsRef,
    addToast: () => {},
    throwNextUpdate: false,
    updateTab: (_id: string, fn: (t: any) => any) => {
      if (ctx.throwNextUpdate) {
        ctx.throwNextUpdate = false;
        throw new Error("render failed");
      }
      tab = fn(tab);
      tabsRef.current = [tab];
      history.push(new Set(tab.dismissedHighlights));
    },
  } as any;
  return { progress: createProgress(ctx), tab: () => tab, ctx };
}

const settle = () => new Promise((r) => setTimeout(r, 80));

describe("dismissal persistence", () => {
  test("back-to-back dismissals keep both keys, write in order, and pick up another writer's key", async () => {
    // The resolve script already wrote "x" to disk; the app's view doesn't have it.
    disk.keys = ["x"];
    disk.resolutions = { x: { state: "intentional" } };
    const { progress, tab } = fakeCtx();
    progress.resolveHighlight("a", null);
    progress.resolveHighlight("b", { state: "noise", reason: "" } as any);
    // The optimistic update already shows both (no stale-snapshot loss).
    expect([...tab().dismissedHighlights].sort()).toEqual(["a", "b"]);
    const shown = history.length;
    await settle();
    // No flicker: the first write's result (which predates "b") is never
    // applied while "b" is still queued; "b" stays shown throughout.
    expect(history.slice(shown).every((keys) => keys.has("b"))).toBe(true);
    expect(calls).toEqual(["dismiss_highlight:a", "dismiss_highlight:b"]);
    // After the writes, the tab shows the merged disk state, script key included.
    expect([...tab().dismissedHighlights].sort()).toEqual(["a", "b", "x"]);
    expect(tab().noteResolutions.get("x")?.state).toBe("intentional");
    expect(disk.keys.sort()).toEqual(["a", "b", "x"]);
  });

  test("restore removes only its key", async () => {
    const { progress, tab } = fakeCtx();
    progress.restoreHighlight("a");
    await settle();
    expect(disk.keys.sort()).toEqual(["b", "x"]);
    expect([...tab().dismissedHighlights].sort()).toEqual(["b", "x"]);
  });

  test("a write whose result can't be applied doesn't jam the writes after it", async () => {
    const { progress, ctx } = fakeCtx();
    progress.resolveHighlight("f1", null);
    // The optimistic update is done; make applying f1's result throw.
    ctx.throwNextUpdate = true;
    await settle();
    progress.resolveHighlight("f2", null);
    await settle();
    expect(calls.slice(-2)).toEqual(["dismiss_highlight:f1", "dismiss_highlight:f2"]);
    expect(disk.keys).toContain("f2");
  });

  test("every dismissal change bumps the tab's version, so an older read can tell", async () => {
    const { progress } = fakeCtx();
    const before = dismissalVersionOf("t1");
    progress.resolveHighlight("v1", null);
    progress.restoreHighlight("v1");
    await settle();
    expect(dismissalVersionOf("t1")).toBe(before + 2);
  });
});
