// What an analysis cost (issue #253), in words for the About panel.
import type { AiUsage } from "../types";

const CONNECTION: Record<string, string> = {
  "claude-cli": "the Claude CLI",
  "anthropic-api": "the Anthropic API",
  bedrock: "AWS Bedrock",
  openai: "OpenAI",
  gemini: "Gemini",
  "openai-compatible": "an OpenAI-compatible API",
};

export function money(usd: number): string {
  if (usd < 0.01) return "<$0.01";
  return `$${usd.toFixed(2)}`;
}

export interface CostLine {
  /** The headline, e.g. "$0.14 · 5 AI calls via the Claude CLI". */
  text: string;
  /** The cheaper path, when there is one worth mentioning. */
  hint: string | null;
}

/** Describe an analysis's AI usage. A reported cost wins over a list-price
 * estimate; with neither, only the call count is known. */
export function describeCost(u: AiUsage): CostLine {
  const via = CONNECTION[u.connection] ?? u.connection;
  const calls = `${u.calls} AI call${u.calls === 1 ? "" : "s"} via ${via}`;
  let text: string;
  if (u.reported_cost_usd != null) text = `${money(u.reported_cost_usd)} · ${calls}`;
  else if (u.list_cost_usd != null) text = `≈${money(u.list_cost_usd)} at list price · ${calls}`;
  else text = `${calls} · cost not reported`;
  let hint: string | null = null;
  const current = u.reported_cost_usd ?? u.list_cost_usd;
  // Only worth saying when an Anthropic key would be meaningfully cheaper.
  if (u.connection === "claude-cli" && u.api_estimate_usd != null && current != null && u.api_estimate_usd < current * 0.8) {
    hint = `≈${money(u.api_estimate_usd)} with an Anthropic key — the CLI adds its own setup to every call.`;
  }
  return { text, hint };
}
