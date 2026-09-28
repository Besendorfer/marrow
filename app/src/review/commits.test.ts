// handleViewCommit (issue #238 phase 6): in the inbox layout, viewing a commit
// from anywhere (About's commit list, Newer/Older, chat) opens the Commits row.
import { describe, expect, test } from "bun:test";
import { createCommits } from "./commits";
import type { PrCommit, Tab } from "../types";

function view(inboxLayout: boolean) {
  let updated = { id: "1", lens: "files", inboxSelection: "file:a.ts", inboxSelectionPath: "a.ts" } as Tab;
  const cached = { sha: "c1", files: [] };
  const ctx = {
    activeTabId: "1",
    activeTabIdRef: { current: "1" },
    inboxLayout,
    tabsRef: { current: [updated] },
    updateTab: (_id: string, fn: (t: Tab) => Tab) => { updated = fn(updated); },
    commitDiffCacheRef: { current: new Map([["c1", cached]]) },
    commitDiffFetchingRef: { current: new Set() },
    setCommitDiff: () => {},
    setCommitDiffError: () => {},
    setCommitDiffLoading: () => {},
  };
  createCommits(ctx).handleViewCommit({ sha: "c1" } as PrCommit);
  return updated;
}

describe("handleViewCommit", () => {
  test("the inbox layout selects the Commits row", () => {
    const t = view(true);
    expect(t.lens).toBe("commits");
    expect(t.selectedCommit?.sha).toBe("c1");
    expect(t.inboxSelection).toBe("commits");
    expect(t.inboxSelectionPath).toBeNull();
  });

  test("the classic layout leaves the selection alone", () => {
    const t = view(false);
    expect(t.lens).toBe("commits");
    expect(t.inboxSelection).toBe("file:a.ts");
  });
});
