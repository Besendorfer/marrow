// listKeyAction (issue #238 phase 4): the inbox list's key guards.
import { describe, expect, test } from "bun:test";
import { listKeyAction } from "./inboxKeys";

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
