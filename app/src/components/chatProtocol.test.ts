// Unit tests for the chat protocol layer (issue #166): the marrow-action /
// marrow-card fence parsers and validators. These are the pure-function
// invariants that keep transcript chips honest — most critically, that the
// execute side (App's streaming effect) and the render side (ChatMarkdown)
// always agree on block indices. First bun-test suite in the repo; run with
// `bun test` from app/.
import { describe, expect, test } from "bun:test";
import { isChatAction, isChatToolCall, parseActionFences, parseChatActionFences, toolChipLabel } from "./RichText";
import { isChatCard, normalizeRow, tableTruncationNote } from "./ChatCards";
import type { ChatAction, ChatToolCall } from "../types";

// ── isChatAction ─────────────────────────────────────────────────────────────

describe("isChatAction", () => {
  test("accepts every documented action shape", () => {
    const valid = [
      { action: "open_file", path: "app/src/App.tsx" },
      { action: "open_file", path: "app/src/App.tsx", line: 42 },
      { action: "open_overview" },
      { action: "next_file" },
      { action: "prev_file" },
      { action: "open_commit", sha: "abc1234" },
      { action: "set_hunk_filter", filter: "all" },
      { action: "set_hunk_filter", filter: "high" },
      { action: "set_hunk_filter", filter: "medium" },
      { action: "set_view_mode", mode: "split" },
      { action: "set_view_mode", mode: "unified" },
      { action: "show_comments", open: true },
      { action: "draft_comment", path: "app/src/App.tsx", line: 42, body: "Consider a guard here." },
      { action: "draft_comment", path: "app/src/App.tsx", line: 42, start_line: 40, body: "This range races." },
      { action: "draft_pr_comment", body: "Nice cleanup overall — one nit inline." },
      // Extra fields are tolerated, consistent with every other action.
      { action: "draft_pr_comment", body: "LGTM", extra: true },
    ];
    for (const a of valid) expect(isChatAction(a)).toBe(true);
  });

  test("rejects near-misses without throwing", () => {
    const invalid = [
      null,
      undefined,
      "open_file",
      [],
      {},
      { action: "open_file" }, // missing path
      { action: "open_file", path: 7 },
      { action: "open_file", path: "x", line: "42" }, // line must be number
      { action: "open_commit" }, // missing sha
      { action: "set_hunk_filter", filter: "med" }, // "med" is not a value
      { action: "set_view_mode", mode: "side-by-side" },
      { action: "show_comments", open: "yes" },
      { action: "resolve_note", key: "x" }, // mutating actions don't exist
      { action: "draft_comment", path: "x.ts", line: 5 }, // missing body
      { action: "draft_comment", path: "x.ts", line: 5, body: "" }, // empty body
      { action: "draft_comment", path: "x.ts", body: "text" }, // missing line
      { action: "draft_comment", path: "", line: 5, body: "text" }, // empty path
      { action: "draft_comment", path: "x.ts", line: 5, start_line: 5, body: "t" }, // start_line must be < line
      { action: "draft_comment", path: "x.ts", line: 5, start_line: 9, body: "t" }, // inverted range
      { action: "draft_comment", path: "x.ts", line: 0, body: "t" }, // line must be positive
      { action: "draft_pr_comment" }, // missing body
      { action: "draft_pr_comment", body: "" }, // empty body
      { action: "draft_pr_comment", body: 7 }, // body must be a string
    ];
    for (const a of invalid) expect(isChatAction(a)).toBe(false);
  });
});

// ── isChatToolCall ───────────────────────────────────────────────────────────

describe("isChatToolCall", () => {
  test("accepts the three documented shapes", () => {
    const valid = [
      { tool: "read_file", path: "app/src/App.tsx" },
      { tool: "search_code", query: "fn foo" },
      { tool: "list_dir", path: "app/src" },
      { tool: "list_dir", path: "" }, // empty path = repo root, allowed
    ];
    for (const t of valid) expect(isChatToolCall(t)).toBe(true);
  });

  test("rejects near-misses without throwing", () => {
    const invalid = [
      null,
      undefined,
      "read_file",
      [],
      {},
      { tool: "read_file" }, // missing path
      { tool: "read_file", path: 7 }, // wrong type
      { tool: "read_file", path: "" }, // empty path not allowed for read_file
      { tool: "search_code" }, // missing query
      { tool: "search_code", query: "" }, // empty query not allowed
      { tool: "list_dir" }, // missing path (no default at the type-guard level)
      { tool: "delete_repo", path: "x" }, // unknown tool
      { action: "open_file", path: "x" }, // marrow-action shape, not a tool
    ];
    for (const t of invalid) expect(isChatToolCall(t)).toBe(false);
  });
});

describe("toolChipLabel", () => {
  test("labels each tool, including root-listing and long-query ellipsis", () => {
    expect(toolChipLabel({ tool: "read_file", path: "app/src/App.tsx" })).toBe("Read App.tsx");
    expect(toolChipLabel({ tool: "search_code", query: "fn foo" })).toBe("Searched “fn foo”");
    expect(toolChipLabel({ tool: "list_dir", path: "app/src" })).toBe("Listed app/src");
    expect(toolChipLabel({ tool: "list_dir", path: "" })).toBe("Listed repo root");

    const longQuery = "x".repeat(50);
    const label = toolChipLabel({ tool: "search_code", query: longQuery } as ChatToolCall);
    expect(label).toContain("…");
    expect(label.length).toBeLessThan(longQuery.length);
  });
});

// ── isChatCard ───────────────────────────────────────────────────────────────

describe("isChatCard", () => {
  const table = {
    type: "table",
    title: "Files",
    columns: ["File", "Adds"],
    rows: [["a.ts", "12"], [{ text: "b.ts", path: "src/b.ts", line: 3 }, "4"]],
  };
  const list = {
    type: "list",
    items: [{ text: "one" }, { text: "two", detail: "d", path: "src/a.ts", line: 9 }],
  };

  test("accepts both documented schemas", () => {
    expect(isChatCard(table)).toBe(true);
    expect(isChatCard(list)).toBe(true);
    expect(isChatCard({ type: "list", items: [] })).toBe(true); // empty is valid
  });

  test("caps are a render concern, not validation", () => {
    const huge = {
      type: "table",
      columns: Array.from({ length: 20 }, (_, i) => `c${i}`),
      rows: Array.from({ length: 200 }, () => Array.from({ length: 20 }, () => "x")),
    };
    expect(isChatCard(huge)).toBe(true);
  });

  test("rejects malformed payloads without throwing", () => {
    const invalid = [
      null,
      "table",
      [],
      { type: "table", columns: ["a"], rows: [["ok"], "not-a-row"] },
      { type: "table", columns: [1, 2], rows: [] },
      { type: "table", items: [{ text: "wrong container" }] }, // list body on table
      { type: "table", columns: ["a"], rows: [[{ path: "x.ts" }]] }, // cell missing text
      { type: "table", columns: ["a"], rows: [[{ text: "t", line: "3" }]] }, // line as string
      { type: "list", items: [{ detail: "no text" }] },
      { type: "list", items: "not-an-array" },
      { type: "chart", series: [] }, // unknown type
      { type: "list", items: [], title: 7 }, // title must be a string
    ];
    for (const c of invalid) expect(isChatCard(c)).toBe(false);
  });
});

// ── fence parsing: the block-index invariant ─────────────────────────────────

const fence = (lang: string, body: string) => "```" + lang + "\n" + body + "\n```";
const ACTION_A: ChatAction = { action: "open_overview" };
const ACTION_B: ChatAction = { action: "set_view_mode", mode: "unified" };
const CARD = { type: "list", items: [{ text: "x" }] };

describe("parseActionFences / parseChatActionFences", () => {
  test("returns only closed marrow-action fences, in order", () => {
    const text = [
      "Some prose.",
      fence("marrow-action", JSON.stringify(ACTION_A)),
      fence("rust", "fn main() {}"), // ordinary code fence — ignored
      fence("marrow-card", JSON.stringify(CARD)), // card — never an action
      fence("marrow-action", JSON.stringify(ACTION_B)),
    ].join("\n");
    const fences = parseActionFences(text);
    expect(fences.length).toBe(2);
    expect(fences[0].action).toEqual(ACTION_A);
    expect(fences[1].action).toEqual(ACTION_B);
  });

  test("an interleaved card fence must not shift action block indices", () => {
    // The execute side keys statuses by index into THIS array; if a card fence
    // ever counted as an action block, chips would show the wrong outcomes.
    const without = parseActionFences(
      [fence("marrow-action", JSON.stringify(ACTION_A)), fence("marrow-action", JSON.stringify(ACTION_B))].join("\n")
    );
    const withCard = parseActionFences(
      [
        fence("marrow-action", JSON.stringify(ACTION_A)),
        fence("marrow-card", JSON.stringify(CARD)),
        fence("marrow-action", JSON.stringify(ACTION_B)),
      ].join("\n")
    );
    expect(withCard.map((f) => f.action)).toEqual(without.map((f) => f.action));
  });

  test("a marrow-tool fence is never picked up as an action", () => {
    // Tool fences are backend-executed and frontend-rendered only — they
    // must not shift action block indices any more than a card fence does.
    const text = [
      fence("marrow-tool", JSON.stringify({ tool: "list_dir", path: "" })),
      fence("marrow-action", JSON.stringify(ACTION_A)),
    ].join("\n");
    const fences = parseActionFences(text);
    expect(fences.length).toBe(1);
    expect(fences[0].action).toEqual(ACTION_A);
  });

  test("an unclosed fence is not executed-parseable", () => {
    const text = "prose\n```marrow-action\n" + JSON.stringify(ACTION_A); // no closing fence
    expect(parseActionFences(text).length).toBe(0);
  });

  test("invalid JSON or unknown action yields action: null (chip falls back)", () => {
    const text = [
      fence("marrow-action", "{ not json"),
      fence("marrow-action", JSON.stringify({ action: "explode" })),
    ].join("\n");
    const fences = parseActionFences(text);
    expect(fences.length).toBe(2);
    expect(fences[0].action).toBeNull();
    expect(fences[1].action).toBeNull();
  });

  test("thought markers do not desync the chat-side parse", () => {
    // ChatMarkdown splits on [[thought:N]] before rendering; the executor uses
    // parseChatActionFences to segment identically. A divider between fences
    // must leave the same actions in the same order.
    const text = [
      fence("marrow-action", JSON.stringify(ACTION_A)),
      "[[thought:1]]",
      "some deliberation",
      "[[thought:1]]",
      fence("marrow-action", JSON.stringify(ACTION_B)),
    ].join("\n");
    const fences = parseChatActionFences(text);
    expect(fences.map((f) => f.action)).toEqual([ACTION_A, ACTION_B]);
  });
});

// ── table truncation footer copy ─────────────────────────────────────────────

describe("normalizeRow", () => {
  test("pads short rows and drops extras to the header width", () => {
    expect(normalizeRow(["a"], 3)).toEqual(["a", "", ""]);
    expect(normalizeRow(["a", "b", "c", "d"], 2)).toEqual(["a", "b"]);
    expect(normalizeRow([], 2)).toEqual(["", ""]);
    const cell = { text: "t", path: "x.ts" };
    expect(normalizeRow([cell], 1)).toEqual([cell]); // objects pass through
  });
});

describe("tableTruncationNote", () => {
  test("names the cap that actually fired", () => {
    expect(tableTruncationNote(51, 3)).toBe("first 50 rows");
    expect(tableTruncationNote(10, 9)).toBe("first 8 columns");
    expect(tableTruncationNote(60, 9)).toBe("first 50 rows, first 8 columns");
    expect(tableTruncationNote(50, 8)).toBe(""); // at the caps exactly — nothing cut
  });
});
