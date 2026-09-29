// describeCost (issue #253).
import { describe, expect, test } from "bun:test";
import { describeCost, money } from "./cost";
import type { AiUsage } from "../types";

const base: AiUsage = {
  connection: "claude-cli", model: "claude-opus-5-5", calls: 5, calls_with_usage: 5,
  input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0,
  content_chars_in: 0, content_chars_out: 0,
};

describe("describeCost", () => {
  test("a CLI analysis shows the reported cost and the cheaper key path", () => {
    const c = describeCost({ ...base, reported_cost_usd: 0.142, api_estimate_usd: 0.039 });
    expect(c.text).toBe("$0.14 · 5 AI calls via the Claude CLI");
    expect(c.hint).toBe("≈$0.04 with an Anthropic key — the CLI adds its own setup to every call.");
  });

  test("no hint when the key wouldn't be meaningfully cheaper", () => {
    expect(describeCost({ ...base, reported_cost_usd: 0.05, api_estimate_usd: 0.045 }).hint).toBeNull();
  });

  test("the Anthropic API shows a list-price estimate and no hint", () => {
    const c = describeCost({ ...base, connection: "anthropic-api", calls: 1, list_cost_usd: 0.004 });
    expect(c.text).toBe("≈<$0.01 at list price · 1 AI call via the Anthropic API");
    expect(c.hint).toBeNull();
  });

  test("without a cost, only the calls are known", () => {
    expect(describeCost({ ...base, connection: "gemini", calls: 3 }).text).toBe("3 AI calls via Gemini · cost not reported");
  });

  test("money rounds to cents and floors tiny amounts", () => {
    expect(money(0.004)).toBe("<$0.01");
    expect(money(1.2345)).toBe("$1.23");
  });
});
