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

const { createProgress } = await import("./progress");

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
    updateTab: (_id: string, fn: (t: any) => any) => {
      tab = fn(tab);
      tabsRef.current = [tab];
    },
  } as any;
  return { progress: createProgress(ctx), tab: () => tab };
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
    await settle();
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
});
