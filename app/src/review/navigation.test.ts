// setSelectedFile's inbox rule (issue #238 phase 4): opening a file from
// anywhere moves the review-list selection to it — unless the selection is
// already anchored to that file (a finding the inbox just opened).
import { describe, expect, test } from "bun:test";
import { createNavigation } from "./navigation";
import type { FileDiff, Tab } from "../types";

function run(tab: Partial<Tab>, file: FileDiff): Tab {
  let updated = { id: "1", lens: "overview", ...tab } as Tab;
  const ctx = {
    activeTabId: "1",
    tabsRef: { current: [updated] },
    updateTab: (_id: string, fn: (t: Tab) => Tab) => {
      updated = fn(updated);
    },
  };
  createNavigation(ctx).setSelectedFile(file);
  return updated;
}

const a = { path: "a.ts" } as FileDiff;
const b = { path: "b.ts" } as FileDiff;

describe("setSelectedFile — inbox selection", () => {
  test("a file opened from elsewhere becomes the selection", () => {
    const t = run({ inboxSelection: "about", inboxSelectionPath: null }, b);
    expect(t.inboxSelection).toBe("file:b.ts");
    expect(t.inboxSelectionPath).toBe("b.ts");
    expect(t.selectedFile).toBe(b);
    expect(t.lens).toBe("files");
  });

  test("a selection anchored to the same file is kept (the finding that opened it)", () => {
    const t = run({ inboxSelection: "risk:a.ts:5:x", inboxSelectionPath: "a.ts" }, a);
    expect(t.inboxSelection).toBe("risk:a.ts:5:x");
  });

  test("a finding anchored to another file gives way to the newly opened file", () => {
    const t = run({ inboxSelection: "risk:a.ts:5:x", inboxSelectionPath: "a.ts" }, b);
    expect(t.inboxSelection).toBe("file:b.ts");
  });
});

// Inbox layout (issue #238 phase 6): About, Commits, and Checks are review-list
// rows, so every lens switch (keyboard 1–4, palette, chat, panel links) lands
// the selection on the matching row.
describe("setLens — inbox selection follows the lens", () => {
  const manifest = { files: [a, b], commits: [{ sha: "c1" }] } as unknown as Tab["manifest"];

  function lens(tab: Partial<Tab>, to: Tab["lens"], inboxLayout = true) {
    let updated = { id: "1", lens: "files", manifest, selectedFile: a, viewedFiles: new Set(), ...tab } as Tab;
    const calls: string[] = [];
    const ctx = {
      activeTabId: "1",
      inboxLayout,
      visibleOrderRef: { current: [] },
      tabsRef: { current: [updated] },
      updateTab: (_id: string, fn: (t: Tab) => Tab) => { updated = fn(updated); },
      handleViewCommit: (c: { sha: string }) => calls.push(`commit:${c.sha}`),
    };
    createNavigation(ctx).setLens("1", to);
    return { t: updated, calls };
  }

  test("Commits, Checks, and Overview select their rows", () => {
    expect(lens({ selectedCommit: { sha: "c1" } as Tab["selectedCommit"] }, "commits").t.inboxSelection).toBe("commits");
    expect(lens({}, "checks").t.inboxSelection).toBe("checks");
    const t = lens({ inboxSelection: "file:a.ts", inboxSelectionPath: "a.ts" }, "overview").t;
    expect(t.inboxSelection).toBe("about");
    expect(t.inboxSelectionPath).toBeNull();
  });

  test("Files lands on the file being shown, keeping a finding anchored to it", () => {
    expect(lens({ lens: "commits", inboxSelection: "commits", inboxSelectionPath: null }, "files").t.inboxSelection).toBe("file:a.ts");
    expect(lens({ lens: "commits", inboxSelection: "risk:a.ts:5:x", inboxSelectionPath: "a.ts" }, "files").t.inboxSelection).toBe("risk:a.ts:5:x");
  });

  test("Commits with no commit picked hands off to handleViewCommit (which selects the row)", () => {
    expect(lens({}, "commits").calls).toEqual(["commit:c1"]);
  });

  test("the classic layout leaves the selection alone", () => {
    const t = lens({ inboxSelection: "file:a.ts" }, "checks", false).t;
    expect(t.lens).toBe("checks");
    expect(t.inboxSelection).toBe("file:a.ts");
  });
});
