// The shared dialog's Tab trap (issue #238 phase 7).
import { describe, expect, test } from "bun:test";
import { trapTarget } from "./Dialog";

describe("trapTarget", () => {
  test("Tab walks forward and wraps from the last to the first", () => {
    expect(trapTarget(3, 0, false)).toBe(1);
    expect(trapTarget(3, 2, false)).toBe(0);
  });

  test("Shift+Tab walks back and wraps from the first to the last", () => {
    expect(trapTarget(3, 2, true)).toBe(1);
    expect(trapTarget(3, 0, true)).toBe(2);
  });

  test("from the dialog itself, Tab enters at the start and Shift+Tab at the end", () => {
    expect(trapTarget(3, -1, false)).toBe(0);
    expect(trapTarget(3, -1, true)).toBe(2);
  });

  test("nothing focusable keeps focus where it is", () => {
    expect(trapTarget(0, -1, false)).toBe(-1);
  });
});
