import { describe, expect, test } from "bun:test";
import { contextRow, highlightCommentBody, prRepoOf } from "../utils";

describe("highlightCommentBody (issue #231)", () => {
  test("note alone when there is no scenario or fix", () => {
    expect(highlightCommentBody({ start_line: 1, end_line: 1, severity: "info", comment: "c" })).toBe("c");
  });

  test("note, then scenario, then fix", () => {
    expect(
      highlightCommentBody({
        start_line: 1,
        end_line: 2,
        severity: "warning",
        comment: "expiry check dropped",
        scenario: "A token cached yesterday is served today",
        fix: "Restore the expires_at comparison",
      }),
    ).toBe(
      "expiry check dropped\n\nA token cached yesterday is served today\n\nSuggested fix: Restore the expires_at comparison",
    );
  });
});

describe("contextRow / prRepoOf (issue #232)", () => {
  const pr = prRepoOf("https://github.com/acme/api/pull/7");
  test("extracts owner/name from a PR URL", () => {
    expect(pr).toBe("acme/api");
    expect(prRepoOf("not a url")).toBe("");
  });
  test("renders each read kind, naming other repos only", () => {
    expect(contextRow({ repo: "acme/api", path: "src/a.rs", rev: "head", tool: "read_file" }, pr)).toBe("Read src/a.rs");
    expect(contextRow({ repo: "acme/api", path: "src/a.rs", rev: "base", tool: "read_file" }, pr)).toBe(
      "Read src/a.rs (before this PR)",
    );
    expect(contextRow({ repo: "acme/web", path: "src/b.ts", rev: "default", tool: "read_file" }, pr)).toBe(
      "Read acme/web · src/b.ts",
    );
    expect(contextRow({ repo: "acme/api", path: "get_user(", rev: "head", tool: "search_code" }, pr)).toBe(
      "Searched for “get_user(”",
    );
    expect(contextRow({ repo: "acme/*", path: "InProgress", rev: "owner", tool: "search_code" }, pr)).toBe(
      "Searched all acme repos for “InProgress”",
    );
    expect(contextRow({ repo: "acme/api", path: "", rev: "head", tool: "list_dir" }, pr)).toBe("Listed repo root");
  });
});
