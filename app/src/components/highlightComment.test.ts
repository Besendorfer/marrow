import { describe, expect, test } from "bun:test";
import { highlightCommentBody } from "../utils";

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
