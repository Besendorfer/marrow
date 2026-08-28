//! Shared HTTP plumbing: one client with sane timeouts, plus the retry and
//! rate-limit vocabulary every network call site speaks (issue #198).
//!
//! The client carries only a CONNECT timeout — streaming AI responses are
//! legitimately unbounded, so overall deadlines are per-request opt-ins
//! (`GITHUB_REQUEST_TIMEOUT`, `AI_REQUEST_TIMEOUT`) at the sites that expect
//! a bounded body.

use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Overall deadline for GitHub REST/GraphQL calls (bounded JSON bodies).
pub const GITHUB_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Overall deadline for non-streaming AI invokes. Generous on purpose: a
/// budget-capped highlights prompt on a slow provider can legitimately run
/// several minutes, and since the highlights pass hard-fails the fetch, a
/// too-tight deadline would turn honest slowness into failure. This bounds
/// hangs, not inference. AI timeouts are NOT retried (see `post_with_retries`).
pub const AI_REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
/// Total attempts (first try + retries) for transient failures.
pub const MAX_ATTEMPTS: u32 = 3;
/// Longest server-directed wait we'll honor inline; beyond this the wait is
/// surfaced to the user instead of silently blocking a pass.
const RETRY_AFTER_CAP: Duration = Duration::from_secs(30);

/// The process-wide HTTP client. Connect timeout only — see module docs.
pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("building the shared reqwest client cannot fail")
    })
}

/// Transport-level failures worth another attempt: the connection never
/// happened or the deadline elapsed. Anything else (TLS, redirect, decode)
/// is not transient.
pub fn transient_transport_error(e: &reqwest::Error) -> bool {
    e.is_connect() || e.is_timeout()
}

/// Connect-only classification for mutations: the request was never sent, so
/// a retry cannot duplicate the side effect.
pub fn unsent_transport_error(e: &reqwest::Error) -> bool {
    e.is_connect()
}

/// If this response is worth retrying, the delay to wait before the next
/// attempt (server-directed when a usable `Retry-After` is present, else the
/// jittered backoff for `attempt`). `None` means don't retry.
pub fn retryable_response_delay(status: StatusCode, headers: &HeaderMap, attempt: u32) -> Option<Duration> {
    let server_wait = retry_after(headers);
    // A secondary rate limit (403 + Retry-After) is retryable; a primary
    // limit (403/429 with the quota exhausted) is not — its reset is minutes
    // to an hour away, which is a message, not a sleep.
    if primary_rate_limited(status, headers) {
        return None;
    }
    let retryable = status.is_server_error()
        || status == StatusCode::TOO_MANY_REQUESTS
        || (status == StatusCode::FORBIDDEN && server_wait.is_some());
    if !retryable {
        return None;
    }
    match server_wait {
        Some(wait) if wait <= RETRY_AFTER_CAP => Some(wait.max(backoff_delay(attempt))),
        Some(_) => None, // server asked for longer than we'll block: surface it
        None => Some(backoff_delay(attempt)),
    }
}

/// Friendly message for a rate-limited response we chose not to wait out —
/// either the primary quota is exhausted (reset-time message) or the server
/// directed a wait longer than `RETRY_AFTER_CAP`. `None` when the response
/// isn't a rate limit at all (callers fall back to the raw body).
pub fn rate_limited_message(status: StatusCode, headers: &HeaderMap) -> Option<String> {
    if primary_rate_limited(status, headers) {
        return Some(rate_limit_message(headers));
    }
    if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
        if let Some(wait) = retry_after(headers) {
            return Some(format!(
                "GitHub asked us to back off — try again in about {} seconds.",
                wait.as_secs()
            ));
        }
    }
    None
}

/// True when the response is GitHub saying the primary rate quota is spent.
pub fn primary_rate_limited(status: StatusCode, headers: &HeaderMap) -> bool {
    (status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS)
        && header_str(headers, "x-ratelimit-remaining") == Some("0")
}

/// Human message for an exhausted primary limit, with the reset time — the
/// "communicate retry timing" half of the rate-limit story.
pub fn rate_limit_message(headers: &HeaderMap) -> String {
    let reset = header_str(headers, "x-ratelimit-reset").and_then(|s| s.parse::<u64>().ok());
    match (reset, now_epoch()) {
        (Some(reset), Some(now)) if reset > now => {
            let mins = (reset - now).div_ceil(60);
            let (h, m) = ((reset % 86_400) / 3_600, (reset % 3_600) / 60);
            format!(
                "GitHub rate limit exceeded — resets at {:02}:{:02} UTC (in about {} minute{}).",
                h, m, mins, if mins == 1 { "" } else { "s" }
            )
        }
        _ => "GitHub rate limit exceeded — it resets within the hour.".to_string(),
    }
}

/// Jittered exponential backoff before the attempt after `attempt` (1-based):
/// 500ms · 2^(attempt-1), plus 0–250ms of time-seeded jitter so concurrent
/// passes don't retry in lockstep.
pub fn backoff_delay(attempt: u32) -> Duration {
    let exp = attempt.saturating_sub(1).min(4);
    Duration::from_millis(500u64.saturating_mul(1 << exp) + jitter_ms(250))
}

fn jitter_ms(range: u64) -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    nanos % range.max(1)
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    header_str(headers, "retry-after")?.parse::<u64>().ok().map(Duration::from_secs)
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

fn now_epoch() -> Option<u64> {
    SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn server_errors_and_429_are_retryable() {
        let h = HeaderMap::new();
        assert!(retryable_response_delay(StatusCode::BAD_GATEWAY, &h, 1).is_some());
        assert!(retryable_response_delay(StatusCode::INTERNAL_SERVER_ERROR, &h, 1).is_some());
        assert!(retryable_response_delay(StatusCode::TOO_MANY_REQUESTS, &h, 1).is_some());
    }

    #[test]
    fn client_errors_are_not_retryable() {
        let h = HeaderMap::new();
        assert!(retryable_response_delay(StatusCode::NOT_FOUND, &h, 1).is_none());
        assert!(retryable_response_delay(StatusCode::UNAUTHORIZED, &h, 1).is_none());
        assert!(retryable_response_delay(StatusCode::FORBIDDEN, &h, 1).is_none());
    }

    #[test]
    fn secondary_limit_retries_within_cap_only() {
        let short = headers(&[("retry-after", "2")]);
        let delay = retryable_response_delay(StatusCode::FORBIDDEN, &short, 1).unwrap();
        assert!(delay >= Duration::from_secs(2));
        let long = headers(&[("retry-after", "120")]);
        assert!(retryable_response_delay(StatusCode::FORBIDDEN, &long, 1).is_none());
    }

    #[test]
    fn exhausted_primary_limit_is_a_message_not_a_retry() {
        let h = headers(&[("x-ratelimit-remaining", "0"), ("retry-after", "2")]);
        assert!(primary_rate_limited(StatusCode::FORBIDDEN, &h));
        assert!(retryable_response_delay(StatusCode::FORBIDDEN, &h, 1).is_none());
        assert!(primary_rate_limited(StatusCode::TOO_MANY_REQUESTS, &h));
        // Quota present ≠ exhausted.
        let ok = headers(&[("x-ratelimit-remaining", "12")]);
        assert!(!primary_rate_limited(StatusCode::FORBIDDEN, &ok));
    }

    #[test]
    fn rate_limit_message_names_the_reset() {
        let reset = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 600;
        let h = headers(&[("x-ratelimit-reset", &reset.to_string())]);
        let msg = rate_limit_message(&h);
        assert!(msg.contains("resets at"), "{msg}");
        assert!(msg.contains("minute"), "{msg}");
        let msg = rate_limit_message(&HeaderMap::new());
        assert!(msg.contains("within the hour"), "{msg}");
    }

    #[test]
    fn long_server_directed_waits_get_a_friendly_message() {
        // Retry-After beyond the inline cap: not retried (tested above), but
        // the surfaced error must still communicate the retry timing.
        let long = headers(&[("retry-after", "120")]);
        let msg = rate_limited_message(StatusCode::FORBIDDEN, &long).unwrap();
        assert!(msg.contains("120 seconds"), "{msg}");
        // Exhausted primary quota still gets the reset-time message.
        let primary = headers(&[("x-ratelimit-remaining", "0")]);
        assert!(rate_limited_message(StatusCode::TOO_MANY_REQUESTS, &primary)
            .unwrap()
            .contains("rate limit exceeded"));
        // A plain 403 (auth failure, SSO) is not a rate limit — raw body path.
        assert!(rate_limited_message(StatusCode::FORBIDDEN, &HeaderMap::new()).is_none());
        assert!(rate_limited_message(StatusCode::NOT_FOUND, &HeaderMap::new()).is_none());
    }

    #[test]
    fn backoff_grows_and_stays_bounded() {
        for attempt in 1..=6 {
            let d = backoff_delay(attempt);
            assert!(d >= Duration::from_millis(500 * (1 << (attempt - 1).min(4))));
            assert!(d <= Duration::from_millis(500 * 16 + 250));
        }
    }
}
