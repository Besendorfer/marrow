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
