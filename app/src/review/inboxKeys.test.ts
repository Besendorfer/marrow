// listKeyAction (issue #238 phase 4): the inbox list's key guards.
import { describe, expect, test } from "bun:test";
import { chooserKeyAction, listKeyAction, nextAfterAction } from "./inboxKeys";

describe("listKeyAction", () => {
  test("j/k and arrows move, with or without a finding selected", () => {
    expect(listKeyAction("j", null)).toEqual({ type: "move", delta: 1 });
    expect(listKeyAction("ArrowDown", { state: "open" })).toEqual({ type: "move", delta: 1 });
    expect(listKeyAction("k", null)).toEqual({ type: "move", delta: -1 });
    expect(listKeyAction("ArrowUp", { state: "checked" })).toEqual({ type: "move", delta: -1 });
  });

  test("e marks only open or commented findings", () => {
    expect(listKeyAction("e", { state: "open" })).toEqual({ type: "fine" });
    expect(listKeyAction("e", { state: "commented" })).toEqual({ type: "fine" });
    expect(listKeyAction("e", { state: "checked" })).toBeNull();
    expect(listKeyAction("e", { state: "dismissed" })).toBeNull();
    expect(listKeyAction("e", null)).toBeNull();
  });

  test("x dismisses anything not already dismissed; c comments on any finding", () => {
    expect(listKeyAction("x", { state: "checked" })).toEqual({ type: "dismiss" });
    expect(listKeyAction("x", { state: "dismissed" })).toBeNull();
    expect(listKeyAction("c", { state: "dismissed" })).toEqual({ type: "comment" });
    expect(listKeyAction("c", null)).toBeNull();
  });

  test("other keys pass through", () => {
    expect(listKeyAction("n", { state: "open" })).toBeNull();
    expect(listKeyAction("Enter", { state: "open" })).toBeNull();
  });
});

describe("nextAfterAction", () => {
  const f = (id: string, state: "open" | "checked" | "commented" | "dismissed" = "open") => ({ id, state });
  const nav = ["about", "a", "b", "c", "file:x", "file:y"];

  test("goes to the next open finding, skipping handled ones", () => {
    expect(nextAfterAction([f("a"), f("b", "checked"), f("c")], nav, "a")).toBe("c");
  });

  test("wraps around to earlier open findings", () => {
    expect(nextAfterAction([f("a"), f("b", "dismissed"), f("c")], nav, "c")).toBe("a");
  });

  test("never lands back on the finding just acted on", () => {
    // Its state is still "open" in this render — the action hasn't committed.
    // No other open finding exists, so it falls through to the next list
    // item ("b") rather than re-selecting "a".
    expect(nextAfterAction([f("a"), f("b", "checked")], nav, "a")).toBe("b");
    expect(nextAfterAction([f("a")], ["about", "a"], "a")).toBeNull();
  });

  test("with no open findings left, moves to the next list item; at the end, nowhere", () => {
    expect(nextAfterAction([f("a", "checked"), f("b", "commented"), f("c")], nav, "c")).toBe("file:x");
    expect(nextAfterAction([f("y")], ["about", "y"], "y")).toBeNull();
  });
});

describe("chooserKeyAction", () => {
  test("digits pick a reason, x or Enter picks the first, Escape cancels", () => {
    expect(chooserKeyAction("2", 3)).toEqual({ type: "pick", index: 1 });
    expect(chooserKeyAction("x", 3)).toEqual({ type: "pick", index: 0 });
    expect(chooserKeyAction("Enter", 3)).toEqual({ type: "pick", index: 0 });
    expect(chooserKeyAction("Escape", 3)).toEqual({ type: "cancel" });
  });

  test("out-of-range digits and other keys aren't picker keys", () => {
    expect(chooserKeyAction("4", 3)).toBeNull();
    expect(chooserKeyAction("0", 3)).toBeNull();
    expect(chooserKeyAction("j", 3)).toBeNull();
  });
});
