//! What an analysis cost (issue #253). Every AI call made inside a
//! [`metered`] scope adds its tokens — and, for the `claude` CLI, the cost it
//! reports — to that scope's meter, so one PR's analysis gets one total even
//! when several tabs analyze at once. Outside a scope, recording is a no-op.
//!
//! Why it matters: through the `claude` CLI every call also carries the CLI's
//! own setup (its system prompt and tools, ~25–35k tokens), which on the
//! corpus cost more than the review itself (#251). An Anthropic key sends only
//! Marrow's prompt, so the summary also estimates that cost for CLI users.

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::{Arc, Mutex};

/// Tokens and cost for one AI call, as the provider reported them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// The provider's own figure (the `claude` CLI reports `total_cost_usd`).
    pub reported_cost_usd: Option<f64>,
}

/// One analysis's AI usage, stored on the manifest.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AiUsage {
    /// The provider label (`ai::Provider::label`): "claude-cli", "anthropic-api", …
    pub connection: String,
    pub model: String,
    pub calls: u32,
    /// Calls whose provider reported token usage (the rest count only as calls).
    pub calls_with_usage: u32,
    /// Calls that failed. A provider may still bill them, but they report no
    /// usage, so the costs above leave them out and the UI says so.
    #[serde(default)]
    pub failed_calls: u32,
    /// Calls Marrow cut short (the agentic review stops a stream at each tool
    /// request). Billed for what streamed, but no usage arrives; left out of
    /// the costs like failed calls, and the UI says so.
    #[serde(default)]
    pub interrupted_calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// Sum of provider-reported costs, when every call reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_cost_usd: Option<f64>,
    /// The same tokens at list price, when the model's price is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_cost_usd: Option<f64>,
    /// For the `claude` CLI: roughly what this analysis would cost with an
    /// Anthropic key — Marrow's own prompts and replies only (≈4 characters
    /// a token), at list price, without the CLI's per-call setup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_estimate_usd: Option<f64>,
    /// Characters of Marrow's own prompts / the replies (feeds the estimate).
    pub content_chars_in: u64,
    pub content_chars_out: u64,
}

/// List prices, USD per million tokens (claude.com/pricing, 2026-09-28).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

/// The list price for a Claude model id, when known. Matched by family so
/// dated ids (`claude-haiku-4-5-20251001`) resolve too. Bare CLI aliases
/// (`opus`, `sonnet`) aren't priced: they follow whatever model the CLI
/// currently maps them to. Opus 5.5's cache read is $0.20, not 10% of its
/// input price — that's the published figure (verified 2026-09-29).
pub fn price_for(model: &str) -> Option<Price> {
    let m = model.to_ascii_lowercase();
    if is_family(&m, "haiku-4-5") {
        Some(Price { input: 1.0, output: 5.0, cache_write: 1.25, cache_read: 0.10 })
    } else if is_family(&m, "sonnet-5") {
        Some(Price { input: 2.0, output: 10.0, cache_write: 2.50, cache_read: 0.20 })
    } else if is_family(&m, "opus-5-5") {
        Some(Price { input: 4.0, output: 20.0, cache_write: 5.0, cache_read: 0.20 })
    } else {
        None
    }
}

/// Whether `model` names exactly `family`: the family may be followed by a
/// date (`-20251001`) or a suffix like `[1m]`, but not by another version
/// number, so `sonnet-5` doesn't price `sonnet-5-5`.
fn is_family(model: &str, family: &str) -> bool {
    model.match_indices(family).any(|(i, _)| {
        let rest = &model[i + family.len()..];
        match rest.strip_prefix('-') {
            Some(tail) => {
                let digits = tail.chars().take_while(|c| c.is_ascii_digit()).count();
                digits >= 6
            }
            None => !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '.'),
        }
    })
}

impl AiUsage {
    fn add(&mut self, call: &CallUsage) {
        self.calls_with_usage += 1;
        self.input_tokens += call.input_tokens;
        self.output_tokens += call.output_tokens;
        self.cache_read_tokens += call.cache_read_tokens;
        self.cache_write_tokens += call.cache_write_tokens;
        self.reported_cost_usd = match (self.calls_with_usage, self.reported_cost_usd, call.reported_cost_usd) {
            (1, _, c) => c,
            (_, Some(total), Some(c)) => Some(total + c),
            // One call didn't report: a partial sum would understate.
            _ => None,
        };
    }

    /// Add another scope's usage (a later AI call on the same analysis, e.g.
    /// re-running requirements coverage) and recompute the derived costs.
    pub fn merged(self, other: AiUsage) -> AiUsage {
        let same_pricing = self.connection == other.connection && self.model == other.model;
        let reported = match (self.reported_cost_usd, other.reported_cost_usd) {
            (Some(a), Some(b)) => Some(a + b),
            _ => None,
        };
        let sum = AiUsage {
            calls: self.calls + other.calls,
            calls_with_usage: self.calls_with_usage + other.calls_with_usage,
            failed_calls: self.failed_calls + other.failed_calls,
            interrupted_calls: self.interrupted_calls + other.interrupted_calls,
            input_tokens: self.input_tokens + other.input_tokens,
            output_tokens: self.output_tokens + other.output_tokens,
            cache_read_tokens: self.cache_read_tokens + other.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens + other.cache_write_tokens,
            content_chars_in: self.content_chars_in + other.content_chars_in,
            content_chars_out: self.content_chars_out + other.content_chars_out,
            reported_cost_usd: reported,
            list_cost_usd: None,
            api_estimate_usd: None,
            ..self
        };
        if !same_pricing {
            // Different providers or models can't be priced together; keep
            // every count but blank every cost rather than mislabel one.
            return AiUsage { reported_cost_usd: None, ..sum };
        }
        sum.finalize()
    }

    /// Fill in the derived costs once the scope is done.
    pub fn finalize(mut self) -> AiUsage {
        let price = price_for(&self.model).filter(|_| matches!(self.connection.as_str(), "claude-cli" | "anthropic-api"));
        if self.calls_with_usage != self.calls {
            // Some calls reported nothing, so any total would understate; or
            // usage was recorded for a call that then failed, so the counts
            // don't line up. Either way, no total rather than a wrong one —
            // the UI says "cost not reported".
            self.reported_cost_usd = None;
        }
        if let Some(p) = price {
            if self.calls_with_usage == self.calls && self.calls > 0 {
                self.list_cost_usd = Some(
                    (self.input_tokens as f64 * p.input
                        + self.output_tokens as f64 * p.output
                        + self.cache_write_tokens as f64 * p.cache_write
                        + self.cache_read_tokens as f64 * p.cache_read)
                        / 1e6,
                );
            }
            if self.connection == "claude-cli" && self.calls > 0 {
                self.api_estimate_usd = Some(
                    (self.content_chars_in as f64 / 4.0 * p.input + self.content_chars_out as f64 / 4.0 * p.output) / 1e6,
                );
            }
        }
        self
    }
}

tokio::task_local! {
    static METER: Arc<Mutex<AiUsage>>;
}

/// Run `fut` with a fresh meter for `connection`/`model`; every AI call inside
/// it (on this task) is counted. Returns the output and the finalized usage.
pub async fn metered<F: Future>(connection: &str, model: &str, fut: F) -> (F::Output, AiUsage) {
    let meter = Arc::new(Mutex::new(AiUsage { connection: connection.to_string(), model: model.to_string(), ..Default::default() }));
    let out = METER.scope(meter.clone(), fut).await;
    let usage = meter.lock().map(|u| u.clone()).unwrap_or_default();
    (out, usage.finalize())
}

/// The current scope's usage so far (finalized), or None outside a scope.
pub fn current() -> Option<AiUsage> {
    METER.try_with(|m| m.lock().map(|u| u.clone()).ok()).ok().flatten().map(AiUsage::finalize)
}

/// Count one call and the characters of Marrow's prompt / the reply.
pub fn record_call(chars_in: usize, chars_out: usize) {
    let _ = METER.try_with(|m| {
        if let Ok(mut u) = m.lock() {
            u.calls += 1;
            u.content_chars_in += chars_in as u64;
            u.content_chars_out += chars_out as u64;
        }
    });
}

/// Marks one call in flight. `finish` it when the call returns; dropped
/// unfinished — the caller abandoned the call mid-stream — it counts as an
/// interrupted call (see `AiUsage::interrupted_calls`).
pub struct CallGuard {
    done: bool,
}

impl CallGuard {
    pub fn start() -> CallGuard {
        CallGuard { done: false }
    }

    pub fn finish(mut self) {
        self.done = true;
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        if !self.done {
            let _ = METER.try_with(|m| {
                if let Ok(mut u) = m.lock() {
                    u.interrupted_calls += 1;
                }
            });
        }
    }
}

/// Count a failed call (no usage to record; see `AiUsage::failed_calls`).
pub fn record_failed_call() {
    let _ = METER.try_with(|m| {
        if let Ok(mut u) = m.lock() {
            u.failed_calls += 1;
        }
    });
}

/// Add a call's provider-reported usage to the current scope.
pub fn record_usage(call: CallUsage) {
    let _ = METER.try_with(|m| {
        if let Ok(mut u) = m.lock() {
            u.add(&call);
        }
    });
}

/// Parse an Anthropic-style `usage` object — the API's responses and the
/// `claude` CLI's result both use these field names.
pub fn usage_from_json(usage: &serde_json::Value, reported_cost: Option<f64>) -> Option<CallUsage> {
    let n = |k: &str| usage.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
    if !usage.is_object() {
        return None;
    }
    Some(CallUsage {
        input_tokens: n("input_tokens"),
        output_tokens: n("output_tokens"),
        cache_read_tokens: n("cache_read_input_tokens"),
        cache_write_tokens: n("cache_creation_input_tokens"),
        reported_cost_usd: reported_cost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prices_resolve_by_family_including_dated_ids() {
        assert_eq!(price_for("claude-haiku-4-5-20251001").unwrap().input, 1.0);
        assert_eq!(price_for("claude-opus-5-5").unwrap().output, 20.0);
        assert!(price_for("gpt-5").is_none());
        assert_eq!(price_for("claude-sonnet-5").unwrap().input, 2.0);
        assert_eq!(price_for("claude-opus-5-5[1m]").unwrap().input, 4.0);
        // A later version of a family isn't priced as the earlier one.
        assert!(price_for("claude-sonnet-5-5").is_none());
        assert!(price_for("claude-sonnet-5.1").is_none());
        assert!(price_for("claude-haiku-4-50").is_none());
    }

    #[test]
    fn a_call_abandoned_mid_stream_counts_as_interrupted() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let (_, u) = rt.block_on(metered("claude-cli", "claude-opus-5-5", async {
            let finished = CallGuard::start();
            record_call(10, 5);
            record_usage(CallUsage { input_tokens: 100, ..Default::default() });
            finished.finish();
            // What `run_agent` does at a tool request: race the stream and
            // drop it when the other branch wins.
            tokio::select! {
                biased;
                _ = async {
                    let _g = CallGuard::start();
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                } => unreachable!(),
                _ = async {} => {}
            }
        }));
        assert_eq!((u.calls, u.calls_with_usage, u.interrupted_calls, u.failed_calls), (1, 1, 1, 0));
        // The finished call is still priced; the UI notes the interrupted one.
        assert!(u.list_cost_usd.is_some());
    }

    #[test]
    fn a_mismatched_merge_keeps_every_count() {
        let a = AiUsage { connection: "claude-cli".into(), model: "m".into(), calls: 2, calls_with_usage: 2, input_tokens: 10, content_chars_in: 40, ..Default::default() };
        let b = AiUsage { connection: "anthropic-api".into(), model: "m".into(), calls: 1, calls_with_usage: 1, input_tokens: 5, interrupted_calls: 1, content_chars_in: 20, ..Default::default() };
        let u = a.merged(b);
        assert_eq!((u.calls, u.input_tokens, u.content_chars_in, u.interrupted_calls), (3, 15, 60, 1));
        assert_eq!((u.reported_cost_usd, u.list_cost_usd, u.api_estimate_usd), (None, None, None));
        assert_eq!(u.connection, "claude-cli");
    }

    #[test]
    fn usage_recorded_for_more_calls_than_succeeded_blanks_the_totals() {
        let u = AiUsage {
            connection: "claude-cli".into(),
            model: "claude-opus-5-5".into(),
            calls: 1,
            calls_with_usage: 2,
            reported_cost_usd: Some(0.5),
            input_tokens: 1000,
            ..Default::default()
        }
        .finalize();
        assert_eq!((u.reported_cost_usd, u.list_cost_usd), (None, None));
    }

    #[test]
    fn a_scope_counts_only_its_own_calls_and_prices_them() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        record_call(10, 10); // outside any scope: ignored
        let (_, u) = rt.block_on(metered("claude-cli", "claude-opus-5-5", async {
            for _ in 0..2 {
                record_call(4_000, 400);
                record_usage(CallUsage {
                    input_tokens: 10,
                    output_tokens: 100,
                    cache_read_tokens: 20_000,
                    cache_write_tokens: 10_000,
                    reported_cost_usd: Some(0.06),
                });
            }
        }));
        assert_eq!((u.calls, u.calls_with_usage, u.output_tokens), (2, 2, 200));
        assert!((u.reported_cost_usd.unwrap() - 0.12).abs() < 1e-9);
        // 20 in ×4 + 200 out ×20 + 20k write ×5 + 40k read ×0.2, per million.
        assert!((u.list_cost_usd.unwrap() - (20.0 * 4.0 + 200.0 * 20.0 + 20_000.0 * 5.0 + 40_000.0 * 0.2) / 1e6).abs() < 1e-9);
        // API estimate: 8k chars in → 2k tokens ×4, 800 chars out → 200 ×20.
        assert!((u.api_estimate_usd.unwrap() - (2_000.0 * 4.0 + 200.0 * 20.0) / 1e6).abs() < 1e-9);
    }

    #[test]
    fn a_call_without_usage_blanks_the_totals_instead_of_understating() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (_, u) = rt.block_on(metered("anthropic-api", "claude-sonnet-5", async {
            record_call(100, 100);
            record_usage(CallUsage { input_tokens: 1_000, output_tokens: 100, ..Default::default() });
            record_call(100, 100); // reported nothing
        }));
        assert_eq!((u.calls, u.calls_with_usage), (2, 1));
        assert!(u.list_cost_usd.is_none());
        assert!(u.reported_cost_usd.is_none());
        assert!(u.api_estimate_usd.is_none(), "only estimated for the CLI");
    }

    #[test]
    fn unknown_models_and_other_providers_get_no_price() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (_, u) = rt.block_on(metered("openai-compatible", "claude-opus-5-5", async {
            record_call(1, 1);
            record_usage(CallUsage { input_tokens: 1, ..Default::default() });
        }));
        assert!(u.list_cost_usd.is_none() && u.api_estimate_usd.is_none());
    }

    #[test]
    fn a_later_call_merges_into_the_recorded_total() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let run = |cost: f64| {
            rt.block_on(metered("claude-cli", "claude-opus-5-5", async move {
                record_call(400, 40);
                record_usage(CallUsage { output_tokens: 10, reported_cost_usd: Some(cost), ..Default::default() });
            }))
            .1
        };
        let total = run(0.5).merged(run(0.25));
        assert_eq!((total.calls, total.calls_with_usage, total.output_tokens), (2, 2, 20));
        assert!((total.reported_cost_usd.unwrap() - 0.75).abs() < 1e-9);
        assert!(total.api_estimate_usd.unwrap() > 0.0 && total.list_cost_usd.is_some());
        // A re-run on another provider or model can't be priced together.
        let mut other = run(0.1);
        other.connection = "anthropic-api".into();
        let mixed = run(0.5).merged(other);
        assert_eq!(mixed.calls, 2);
        assert!(mixed.reported_cost_usd.is_none() && mixed.list_cost_usd.is_none() && mixed.api_estimate_usd.is_none());
    }

    #[test]
    fn failed_calls_are_counted_apart_and_leave_the_cost_of_the_rest() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (_, u) = rt.block_on(metered("claude-cli", "claude-opus-5-5", async {
            record_call(100, 10);
            record_usage(CallUsage { output_tokens: 5, reported_cost_usd: Some(0.2), ..Default::default() });
            record_failed_call();
        }));
        assert_eq!((u.calls, u.failed_calls), (1, 1));
        assert_eq!(u.reported_cost_usd, Some(0.2));
    }

    #[test]
    fn usage_json_uses_anthropic_field_names() {
        let u = usage_from_json(
            &json!({"input_tokens": 3, "output_tokens": 4, "cache_read_input_tokens": 5, "cache_creation_input_tokens": 6}),
            Some(0.1),
        )
        .unwrap();
        assert_eq!((u.input_tokens, u.output_tokens, u.cache_read_tokens, u.cache_write_tokens), (3, 4, 5, 6));
        assert!(usage_from_json(&json!(null), None).is_none());
    }
}
