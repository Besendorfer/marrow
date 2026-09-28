// Resizable pane widths (issue #238 phase 7).
import { describe, expect, test } from "bun:test";
import { clampWidth, readStoredWidth } from "./usePaneWidth";

const bounds = { min: 260, max: 640, initial: 340 };

describe("pane widths", () => {
  test("drags and nudges stay inside the bounds", () => {
    expect(clampWidth(100, bounds)).toBe(260);
    expect(clampWidth(900, bounds)).toBe(640);
    expect(clampWidth(401.6, bounds)).toBe(402);
  });

  test("a stored width is clamped; a missing or garbled one falls back to the default", () => {
    expect(readStoredWidth("500", bounds)).toBe(500);
    expect(readStoredWidth("9999", bounds)).toBe(640);
    expect(readStoredWidth(null, bounds)).toBe(340);
    expect(readStoredWidth("wide", bounds)).toBe(340);
  });
});
