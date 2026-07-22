use crate::bedrock::BedrockClient;
use crate::types::Settings;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// OpenAI's default API base; also the shape OpenRouter / Groq / Together / many
/// local servers (Ollama, LM Studio) speak.
const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
/// Google Gemini's OpenAI-compatible endpoint.
const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";

/// Extract a JSON array from text that may contain markdown fences or preamble.
pub fn extract_json_array(text: &str) -> Result<serde_json::Value, String> {
    // Try direct parse
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(text.trim()) {
        if val.is_array() {
            return Ok(val);
        }
    }

    // Try to find JSON array in the text
    if let Some(start) = text.find('[') {
        if let Some(end) = text.rfind(']') {
            if end > start {
                let candidate = &text[start..=end];
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(candidate) {
                    if val.is_array() {
                        return Ok(val);
                    }
                }
            }
        }
    }

    // Try stripping markdown code fences
    let stripped = text
        .replace("```json", "")
        .replace("```", "");
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(stripped.trim()) {
        if val.is_array() {
            return Ok(val);
        }
    }

    Err(format!(
        "Could not extract JSON array from AI response. Raw response:\n{}",
        &text[..text.len().min(500)]
    ))
}

/// The AI provider a config resolves to. A pure decision, separated from
/// construction so it's testable without hitting AWS/network.
#[derive(Debug, PartialEq, Eq)]
pub enum Provider {
    Bedrock,
    Anthropic,
    ClaudeCli,
    OpenAi,
    Gemini,
    /// Generic OpenAI-compatible endpoint (OpenRouter, a local server, …).
    OpenAiCompatible,
}

impl Provider {
    /// Human-readable label for `marrow settings`.
    pub fn label(&self) -> &'static str {
        match self {
            Provider::Bedrock => "bedrock",
            Provider::Anthropic => "anthropic-api",
            Provider::ClaudeCli => "claude-cli",
            Provider::OpenAi => "openai",
            Provider::Gemini => "gemini",
            Provider::OpenAiCompatible => "openai-compatible",
        }
    }
}

/// Resolve the provider. An explicit `provider_override` wins; otherwise
/// auto-detect: `arn:` → Bedrock; a custom base URL → OpenAI-compatible;
/// `gpt`/`o1`/`o3`/`o4`/`chatgpt` → OpenAI; `gemini` → Gemini; else (claude*/
/// unknown) → Anthropic if a key is present, else the `claude` CLI.
pub fn choose_provider(
    model: &str,
    provider_override: &str,
    has_base_url: bool,
    has_anthropic_key: bool,
) -> Provider {
    match provider_override.trim().to_ascii_lowercase().as_str() {
        "anthropic" => return Provider::Anthropic,
        "openai" => return Provider::OpenAi,
        "gemini" | "google" => return Provider::Gemini,
        "bedrock" => return Provider::Bedrock,
        "claude-cli" | "claude_cli" | "cli" => return Provider::ClaudeCli,
        "openai-compatible" | "compatible" | "openrouter" => return Provider::OpenAiCompatible,
        _ => {} // empty or unrecognized → fall through to auto-detect
    }

    if model.starts_with("arn:") {
        return Provider::Bedrock;
    }
    if has_base_url {
        return Provider::OpenAiCompatible;
    }
    let m = model.to_ascii_lowercase();
    if ["gpt", "o1", "o3", "o4", "chatgpt"].iter().any(|p| m.starts_with(p)) {
        return Provider::OpenAi;
    }
    if m.starts_with("gemini") {
        return Provider::Gemini;
    }
    if has_anthropic_key {
        Provider::Anthropic
    } else {
        Provider::ClaudeCli
    }
}

/// The provider a full `Settings` resolves to (reads keys/base-url from config +
/// env). Used for construction and for the `marrow settings` display.
pub fn provider_for_settings(s: &Settings) -> Provider {
    let has_anthropic = crate::config::resolve_anthropic_api_key(s).is_some();
    let has_base_url = crate::config::resolve_openai_base_url(s).is_some();
    choose_provider(&s.model, &s.provider, has_base_url, has_anthropic)
}

/// AI backend that dispatches to AWS Bedrock, the Anthropic API, an
/// OpenAI-compatible endpoint (OpenAI / Gemini / OpenRouter / local), or the
/// `claude` CLI. See [`choose_provider`] for the dispatch rule.
pub enum AiBackend {
    Bedrock {
        client: BedrockClient,
        model_arn: String,
    },
    Anthropic {
        api_key: String,
        model: String,
    },
    OpenAiCompatible {
        base_url: String,
        api_key: String,
        model: String,
    },
    ClaudeCli {
        model: String,
    },
}

/// Who authored a turn in a chat conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

impl ChatRole {
    /// The wire string used by the Anthropic / OpenAI message APIs.
    fn api_str(&self) -> &'static str {
        match self {
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
        }
    }
}

/// One turn of a multi-turn chat conversation fed to [`AiBackend::invoke_chat_stream`].
#[derive(Debug, Clone)]
pub struct ChatTurn {
    pub role: ChatRole,
    pub content: String,
}

/// An incremental update from a streaming chat call.
#[derive(Debug, Clone)]
pub enum StreamUpdate {
    /// A fragment of answer text to append.
    Delta(String),
    /// A transient status while no answer text is being produced — e.g. the
    /// `claude` CLI agent using tools between text blocks. `Some(label)` to show,
    /// `None` to clear. Not part of the saved answer.
    Status(Option<String>),
}

impl AiBackend {
    /// Build a backend from a full `Settings` (resolving keys/base-url from
    /// config + env).
    pub async fn from_settings(s: &Settings) -> Result<Self, String> {
        match provider_for_settings(s) {
            Provider::Bedrock => {
                let region = crate::bedrock::region_from_arn(&s.model)?;
                let client = BedrockClient::new(&region, &s.aws_profile).await?;
                Ok(AiBackend::Bedrock { client, model_arn: s.model.clone() })
            }
            Provider::Anthropic => {
                let api_key = crate::config::resolve_anthropic_api_key(s).ok_or(
                    "Anthropic provider selected but no API key. Set anthropic_api_key or ANTHROPIC_API_KEY (or install the `claude` CLI).",
                )?;
                Ok(AiBackend::Anthropic { api_key, model: s.model.clone() })
            }
            Provider::ClaudeCli => Ok(AiBackend::ClaudeCli { model: s.model.clone() }),
            Provider::OpenAi => Self::openai_compatible(
                s,
                OPENAI_BASE_URL,
                crate::config::resolve_openai_api_key(s),
                "OpenAI",
                "OPENAI_API_KEY",
            ),
            Provider::Gemini => Self::openai_compatible(
                s,
                GEMINI_BASE_URL,
                crate::config::resolve_gemini_api_key(s),
                "Gemini",
                "GEMINI_API_KEY",
            ),
            Provider::OpenAiCompatible => {
                let base_url = crate::config::resolve_openai_base_url(s).ok_or(
                    "openai-compatible provider selected but no base URL. Set openai_base_url or OPENAI_BASE_URL.",
                )?;
                let base_url = base_url.trim_end_matches('/').to_string();
                Self::openai_compatible_with_base(s, base_url, crate::config::resolve_openai_api_key(s))
            }
        }
    }

    /// Helper: an OpenAI-compatible backend at a fixed `base_url` (OpenAI/Gemini).
    fn openai_compatible(
        s: &Settings,
        base_url: &str,
        api_key: Option<String>,
        name: &str,
        env_var: &str,
    ) -> Result<Self, String> {
        let api_key = api_key.ok_or_else(|| {
            format!("{name} provider selected but no API key. Set the key in config or {env_var}.")
        })?;
        Ok(AiBackend::OpenAiCompatible { base_url: base_url.to_string(), api_key, model: s.model.clone() })
    }

    /// Helper: an OpenAI-compatible backend at a custom `base_url`.
    fn openai_compatible_with_base(
        s: &Settings,
        base_url: String,
        api_key: Option<String>,
    ) -> Result<Self, String> {
        let api_key = api_key.ok_or(
            "openai-compatible provider needs an API key. Set openai_api_key or OPENAI_API_KEY.",
        )?;
        Ok(AiBackend::OpenAiCompatible { base_url, api_key, model: s.model.clone() })
    }

    /// Send a prompt to the AI and return the text response.
    pub async fn invoke(&self, prompt: &str) -> Result<String, String> {
        match self {
            AiBackend::Bedrock { client, model_arn } => {
                client.invoke_model(model_arn, prompt).await
            }
            AiBackend::Anthropic { api_key, model } => {
                invoke_anthropic(api_key, model, prompt).await
            }
            AiBackend::OpenAiCompatible { base_url, api_key, model } => {
                invoke_openai_compatible(base_url, api_key, model, prompt).await
            }
            AiBackend::ClaudeCli { model } => invoke_claude_cli(model, prompt).await,
        }
    }

    /// Stream a multi-turn chat completion. `system` is the grounding/context
    /// preamble; `turns` is the conversation so far (last turn is the user's new
    /// message). Each text fragment is passed to `on_delta` as it arrives; the
    /// fully assembled text is also returned.
    pub async fn invoke_chat_stream(
        &self,
        system: &str,
        turns: &[ChatTurn],
        on: &mut (dyn FnMut(StreamUpdate) + Send),
    ) -> Result<String, String> {
        match self {
            AiBackend::Bedrock { client, model_arn } => {
                client.converse_stream(model_arn, system, turns, on).await
            }
            AiBackend::Anthropic { api_key, model } => {
                stream_anthropic(api_key, model, system, turns, on).await
            }
            AiBackend::OpenAiCompatible { base_url, api_key, model } => {
                stream_openai_compatible(base_url, api_key, model, system, turns, on).await
            }
            AiBackend::ClaudeCli { model } => {
                stream_claude_cli(model, system, turns, on).await
            }
        }
    }
}

/// Consume a Server-Sent Events response, calling `extract` on each `data:`
/// payload to pull out a text delta. Stops on the OpenAI `[DONE]` sentinel or
/// end of stream. Bytes are buffered and only split on `\n` (which never falls
/// inside a multibyte UTF-8 char), so deltas are never corrupted across chunks.
async fn consume_sse(
    resp: reqwest::Response,
    mut extract: impl FnMut(&str) -> Option<String>,
    on: &mut (dyn FnMut(StreamUpdate) + Send),
) -> Result<String, String> {
    use futures::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut full = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("AI stream read failed: {e}"))?;
        buf.extend_from_slice(&chunk);
        if sse_drain_lines(&mut buf, &mut full, &mut extract, on) {
            return Ok(full);
        }
    }
    // A spec-compliant stream terminates every line with \n, but if the server
    // closes with a trailing partial line, don't silently drop its delta.
    if !buf.is_empty() {
        buf.push(b'\n');
        sse_drain_lines(&mut buf, &mut full, &mut extract, on);
    }
    Ok(full)
}

/// Drain complete `\n`-terminated lines from `buf`, feeding each `data:`
/// payload through `extract` and appending deltas to `full`. Returns true when
/// the `[DONE]` sentinel is seen.
fn sse_drain_lines(
    buf: &mut Vec<u8>,
    full: &mut String,
    extract: &mut impl FnMut(&str) -> Option<String>,
    on: &mut (dyn FnMut(StreamUpdate) + Send),
) -> bool {
    while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
        let line_bytes: Vec<u8> = buf.drain(..=nl).collect();
        let line = String::from_utf8_lossy(&line_bytes);
        let line = line.trim_end_matches(['\r', '\n']);
        let Some(data) = line.strip_prefix("data:") else { continue };
        let data = data.trim();
        if data == "[DONE]" {
            return true;
        }
        if let Some(delta) = extract(data) {
            if !delta.is_empty() {
                full.push_str(&delta);
                on(StreamUpdate::Delta(delta));
            }
        }
    }
    false
}

/// Build the OpenAI/Anthropic-style `messages` array from a system preamble and
/// the conversation turns. The `system` role is prepended (OpenAI convention);
/// Anthropic passes `system` separately and ignores this prepend.
fn turns_to_messages(turns: &[ChatTurn]) -> Vec<serde_json::Value> {
    turns
        .iter()
        .map(|t| serde_json::json!({ "role": t.role.api_str(), "content": t.content }))
        .collect()
}

/// Stream from the Anthropic Messages API (`stream: true`, SSE).
async fn stream_anthropic(
    api_key: &str,
    model: &str,
    system: &str,
    turns: &[ChatTurn],
    on: &mut (dyn FnMut(StreamUpdate) + Send),
) -> Result<String, String> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": ANTHROPIC_MAX_TOKENS,
        "stream": true,
        "system": system,
        "messages": turns_to_messages(turns),
    });
    let resp = reqwest::Client::new()
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Anthropic API request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let snippet: String = text.chars().take(500).collect();
        return Err(format!(
            "Anthropic API error ({status}): {snippet}. Check your ANTHROPIC_API_KEY and that `{model}` is a valid model name."
        ));
    }

    consume_sse(
        resp,
        |data| {
            let json: serde_json::Value = serde_json::from_str(data).ok()?;
            // content_block_delta events carry the streamed text.
            if json["type"] == "content_block_delta" {
                return json["delta"]["text"].as_str().map(str::to_string);
            }
            None
        },
        on,
    )
    .await
}

/// Stream from an OpenAI-compatible Chat Completions endpoint (`stream: true`).
async fn stream_openai_compatible(
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    turns: &[ChatTurn],
    on: &mut (dyn FnMut(StreamUpdate) + Send),
) -> Result<String, String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut messages = vec![serde_json::json!({ "role": "system", "content": system })];
    messages.extend(turns_to_messages(turns));
    let body = serde_json::json!({
        "model": model,
        "stream": true,
        "messages": messages,
    });
    let resp = reqwest::Client::new()
        .post(&url)
        .header("authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("OpenAI-compatible request to {url} failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let snippet: String = text.chars().take(500).collect();
        return Err(format!(
            "OpenAI-compatible API error ({status}) from {url}: {snippet}. Check the API key, base URL, and that `{model}` is valid there."
        ));
    }

    consume_sse(
        resp,
        |data| {
            let json: serde_json::Value = serde_json::from_str(data).ok()?;
            json["choices"][0]["delta"]["content"].as_str().map(str::to_string)
        },
        on,
    )
    .await
}

/// Stream from the `claude` CLI. The CLI is single-shot over stdin, so we flatten
/// the system preamble + turns into one prompt and read stdout incrementally,
/// emitting each chunk as a delta.
async fn stream_claude_cli(
    model: &str,
    system: &str,
    turns: &[ChatTurn],
    on: &mut (dyn FnMut(StreamUpdate) + Send),
) -> Result<String, String> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};

    let mut prompt = String::new();
    prompt.push_str("System instructions:\n");
    prompt.push_str(system);
    prompt.push_str("\n\n");
    for turn in turns {
        let label = match turn.role {
            ChatRole::User => "User",
            ChatRole::Assistant => "Assistant",
        };
        prompt.push_str(&format!("{label}: {}\n\n", turn.content));
    }
    prompt.push_str("Assistant:");

    // `stream-json` + `--include-partial-messages` makes the CLI emit NDJSON with
    // token-level `content_block_delta` events. Plain `--print` buffers the whole
    // answer and prints it at the end (no visible streaming), so we parse the
    // event stream instead.
    let mut child = Command::new(resolve_claude_binary())
        .args([
            "--model", model,
            "--print",
            "--output-format", "stream-json",
            "--include-partial-messages",
            "--verbose",
        ])
        .env("CLAUDECODE", "") // prevent recursive Claude Code invocation
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // If this future is dropped mid-stream (the chat was cancelled), kill the
        // child rather than leaking a running `claude` process.
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            format!(
                "Failed to run claude CLI: {} (os error {}). Is the `claude` command installed and on your PATH?",
                e, e.raw_os_error().unwrap_or(-1)
            )
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|e| format!("Failed to write prompt to claude stdin: {}", e))?;
        // Drop stdin to close it, signaling EOF.
    }

    // Read NDJSON line by line. The CLI runs as an agent: it emits a text block,
    // may call tools, then emits another text block. We stream `text_delta`s as
    // answer text, insert a blank line between separate text blocks (different
    // `index`) so they don't run together, and surface a transient "Working…"
    // status while the agent uses tools between blocks. Splitting on `\n` never
    // bisects a multibyte char, so each complete line decodes cleanly.
    let mut full = String::new();
    // The CLI resets the content-block index to 0 for each new assistant message,
    // so a tool call followed by more text reuses index 0 — index alone can't tell
    // blocks apart. Instead, a new *text block start* after we've already emitted
    // text means a gap (tool use / thinking) happened. When text resumes we insert
    // a `[[thought:<secs>]]` marker (the frontend renders it as a dim "Thought for
    // Xs" divider), timing the gap from the last emitted text token.
    let mut emitted_text = false;
    let mut need_separator = false;
    let mut last_text_at: Option<std::time::Instant> = None;
    let mut cli_error: Option<String> = None;
    if let Some(stdout) = child.stdout.take() {
        let mut reader = tokio::io::BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader
                .read_line(&mut line)
                .await
                .map_err(|e| format!("Failed to read claude stdout: {}", e))?;
            if n == 0 {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match cli_event(trimmed) {
                Some(CliEvent::TextStart) => {
                    if emitted_text {
                        need_separator = true;
                    }
                }
                Some(CliEvent::Text(text)) => {
                    if !text.is_empty() {
                        if need_separator {
                            let secs = last_text_at
                                .map(|t| t.elapsed().as_secs_f64().round() as u64)
                                .unwrap_or(0)
                                .max(1);
                            let marker = format!("\n\n[[thought:{secs}]]\n\n");
                            full.push_str(&marker);
                            on(StreamUpdate::Delta(marker));
                            need_separator = false;
                        }
                        full.push_str(&text);
                        emitted_text = true;
                        last_text_at = Some(std::time::Instant::now());
                        on(StreamUpdate::Delta(text));
                    }
                }
                Some(CliEvent::ToolUse) => {
                    on(StreamUpdate::Status(Some("Working…".to_string())));
                }
                Some(CliEvent::Error(msg)) => {
                    cli_error = Some(msg);
                }
                None => {}
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("Failed to wait for claude CLI: {}", e))?;

    // The CLI reports failures as a JSON error event on stdout (captured above),
    // not via stderr — prefer that message; fall back to stderr, then exit code.
    if let Some(msg) = cli_error {
        return Err(format!("claude CLI error: {msg}"));
    }
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut stderr).await;
        }
        let detail = stderr.trim();
        return Err(if detail.is_empty() {
            format!("claude CLI exited with {status} (no error output). Check the model name in settings and that you're signed in to the claude CLI.")
        } else {
            format!("claude CLI exited with {status}: {detail}")
        });
    }
    if full.trim().is_empty() {
        return Err("claude CLI returned an empty response".to_string());
    }
    Ok(full)
}

/// A meaningful event parsed from one NDJSON line of `claude --output-format
/// stream-json --include-partial-messages`.
#[derive(Debug, PartialEq)]
enum CliEvent {
    /// A text content block began (used to separate consecutive text segments).
    TextStart,
    /// A chunk of answer text.
    Text(String),
    /// The agent started using a tool (a gap between text blocks).
    ToolUse,
    /// The CLI reported a failure (e.g. bad model, auth). The message is for the
    /// user — the CLI emits this on stdout as JSON, not stderr.
    Error(String),
}

/// Parse one NDJSON line into a [`CliEvent`], or None for lines we don't surface
/// (init, usage, thinking deltas/blocks, successful result, …).
fn cli_event(line: &str) -> Option<CliEvent> {
    let json: serde_json::Value = serde_json::from_str(line).ok()?;
    match json["type"].as_str()? {
        "stream_event" => {
            let event = &json["event"];
            match event["type"].as_str()? {
                "content_block_start" => match event["content_block"]["type"].as_str()? {
                    "text" => Some(CliEvent::TextStart),
                    "tool_use" => Some(CliEvent::ToolUse),
                    _ => None, // thinking / other blocks
                },
                "content_block_delta" if event["delta"]["type"] == "text_delta" => {
                    Some(CliEvent::Text(event["delta"]["text"].as_str()?.to_string()))
                }
                _ => None,
            }
        }
        // A result flagged as an error carries a human message in `result`.
        "result" if json["is_error"] == true => Some(CliEvent::Error(
            json["result"].as_str().unwrap_or("the CLI reported an error").to_string(),
        )),
        // An assistant turn can carry a top-level `error` code with a text block.
        "assistant" if json["error"].is_string() => Some(CliEvent::Error(
            json["message"]["content"][0]["text"]
                .as_str()
                .or_else(|| json["error"].as_str())
                .unwrap_or("the CLI reported an error")
                .to_string(),
        )),
        _ => None,
    }
}

/// End-to-end provider check for setup/settings: build the backend from the
/// candidate settings and send a one-word prompt. Returns the provider label
/// on success so the UI can say what actually answered.
pub async fn validate_provider(s: &Settings) -> Result<String, String> {
    let label = provider_for_settings(s).label().to_string();
    let backend = AiBackend::from_settings(s).await?;
    backend
        .invoke("Reply with the single word: ok")
        .await
        .map_err(|e| format!("{label}: {e}"))?;
    Ok(label)
}

/// Upper bound on generated tokens for the Anthropic API (required by the API;
/// generous so classification of large PRs isn't truncated).
const ANTHROPIC_MAX_TOKENS: u32 = 8192;

/// Call the Anthropic Messages API directly (no AWS, no CLI). A single user
/// message, mirroring the Bedrock/CLI paths.
async fn invoke_anthropic(api_key: &str, model: &str, prompt: &str) -> Result<String, String> {
    let body = serde_json::json!({
        "model": model,
        "max_tokens": ANTHROPIC_MAX_TOKENS,
        "messages": [{ "role": "user", "content": prompt }],
    });
    let resp = reqwest::Client::new()
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Anthropic API request failed: {e}"))?;

    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("Anthropic API read failed: {e}"))?;
    if !status.is_success() {
        let snippet: String = text.chars().take(500).collect();
        return Err(format!(
            "Anthropic API error ({status}): {snippet}. Check your ANTHROPIC_API_KEY and that `{model}` is a valid model name."
        ));
    }

    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Anthropic API returned invalid JSON: {e}"))?;
    // `content` is an array of blocks; concatenate the text blocks.
    let out: String = json["content"]
        .as_array()
        .map(|blocks| {
            blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("")
        })
        .unwrap_or_default();
    if out.trim().is_empty() {
        let snippet: String = text.chars().take(500).collect();
        return Err(format!("Anthropic API returned no text content. Raw: {snippet}"));
    }
    Ok(out)
}

/// Call an OpenAI-compatible Chat Completions endpoint (OpenAI, Gemini's compat
/// endpoint, OpenRouter, local servers). A single user message; `max_tokens` is
/// omitted for the widest compatibility (incl. reasoning models).
async fn invoke_openai_compatible(
    base_url: &str,
    api_key: &str,
    model: &str,
    prompt: &str,
) -> Result<String, String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": prompt }],
    });
    let resp = reqwest::Client::new()
        .post(&url)
        .header("authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("OpenAI-compatible request to {url} failed: {e}"))?;

    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("OpenAI-compatible read failed: {e}"))?;
    if !status.is_success() {
        let snippet: String = text.chars().take(500).collect();
        return Err(format!(
            "OpenAI-compatible API error ({status}) from {url}: {snippet}. Check the API key, base URL, and that `{model}` is valid there."
        ));
    }

    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("OpenAI-compatible API returned invalid JSON: {e}"))?;
    let out = json["choices"][0]["message"]["content"].as_str().unwrap_or_default().to_string();
    if out.trim().is_empty() {
        let snippet: String = text.chars().take(500).collect();
        return Err(format!("OpenAI-compatible API returned no content. Raw: {snippet}"));
    }
    Ok(out)
}

/// Resolve the `claude` binary path.  Bundled macOS `.app` bundles inherit a
/// minimal PATH that usually does not include user-local install directories,
/// so we probe well-known locations when a plain `which`-style lookup fails.
///
/// The result is cached for the lifetime of the process via `OnceLock`.
fn resolve_claude_binary() -> &'static str {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    static CLAUDE_BIN: OnceLock<String> = OnceLock::new();
    CLAUDE_BIN.get_or_init(|| {
        if let Ok(output) = std::process::Command::new("which")
            .arg("claude")
            .output()
        {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return path;
                }
            }
        }

        let home = std::env::var("HOME").unwrap_or_default();
        let candidates: Vec<PathBuf> = vec![
            PathBuf::from(&home).join(".local/bin/claude"),
            PathBuf::from("/opt/homebrew/bin/claude"),
            PathBuf::from("/usr/local/bin/claude"),
            PathBuf::from(&home).join(".nvm/current/bin/claude"),
            PathBuf::from(&home).join(".volta/bin/claude"),
        ];

        for candidate in candidates {
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
        }

        "claude".to_string()
    })
}

async fn invoke_claude_cli(model: &str, prompt: &str) -> Result<String, String> {
    // No model configured → let the CLI use its own default. `--model ""` is a
    // 400 from the API.
    let mut args: Vec<&str> = Vec::new();
    if !model.is_empty() {
        args.extend(["--model", model]);
    }
    args.push("--print");
    let mut child = Command::new(resolve_claude_binary())
        .args(&args)
        .env("CLAUDECODE", "") // prevent recursive Claude Code invocation
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "Failed to run claude CLI: {} (os error {}). Is the `claude` command installed and on your PATH?",
                e, e.raw_os_error().unwrap_or(-1)
            )
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|e| format!("Failed to write prompt to claude stdin: {}", e))?;
        // Drop stdin to close it, signaling EOF
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("Failed to wait for claude CLI: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "claude CLI exited with {}: {}{}",
            output.status,
            stderr.trim(),
            if !stdout.is_empty() {
                format!("\nstdout: {}", &stdout[..stdout.len().min(500)])
            } else {
                String::new()
            }
        ));
    }

    let text = String::from_utf8_lossy(&output.stdout).to_string();
    if text.trim().is_empty() {
        return Err("claude CLI returned empty response".to_string());
    }

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    // (model, override, has_base_url, has_anthropic_key)
    fn pick(model: &str, ov: &str, base: bool, anthropic: bool) -> Provider {
        choose_provider(model, ov, base, anthropic)
    }

    #[test]
    fn cli_event_classifies_text_textstart_tool_and_ignores_rest() {
        let d = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"Hello"}}}"#;
        assert_eq!(cli_event(d), Some(CliEvent::Text("Hello".to_string())));
        let text_start = r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}"#;
        assert_eq!(cli_event(text_start), Some(CliEvent::TextStart));
        let tool = r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","name":"Read"}}}"#;
        assert_eq!(cli_event(tool), Some(CliEvent::ToolUse));
        // thinking blocks/deltas, non-stream events, and init/result lines yield nothing.
        let thinking_start = r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}}"#;
        assert_eq!(cli_event(thinking_start), None);
        let thinking = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}}"#;
        assert_eq!(cli_event(thinking), None);
        let init = r#"{"type":"system","subtype":"init"}"#;
        assert_eq!(cli_event(init), None);
        // A successful result is not an error.
        let result = r#"{"type":"result","is_error":false,"result":"Hello"}"#;
        assert_eq!(cli_event(result), None);
        assert_eq!(cli_event("not json"), None);
    }

    #[test]
    fn cli_event_surfaces_error_messages() {
        // The CLI puts the human-readable failure in `result` on an is_error event.
        let err_result = r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":404,"result":"There's an issue with the selected model (foo)."}"#;
        assert_eq!(
            cli_event(err_result),
            Some(CliEvent::Error("There's an issue with the selected model (foo).".to_string()))
        );
        // An assistant turn carrying a top-level error code, with the text block.
        let err_assistant = r#"{"type":"assistant","error":"model_not_found","message":{"content":[{"type":"text","text":"It may not exist or you may not have access."}]}}"#;
        assert_eq!(
            cli_event(err_assistant),
            Some(CliEvent::Error("It may not exist or you may not have access.".to_string()))
        );
    }

    // The agent reuses content-block index 0 across messages, so a tool call
    // between two text blocks looks like (TextStart, Text, ToolUse, TextStart,
    // Text) with the same index — the separator must come from TextStart, not
    // the index. This mirrors the loop logic in `stream_claude_cli`, using a
    // fixed duration in place of the wall-clock timing.
    #[test]
    fn thought_marker_inserted_between_text_blocks_across_a_tool_gap() {
        let events = [
            CliEvent::TextStart,
            CliEvent::Text("first.".to_string()),
            CliEvent::ToolUse,
            CliEvent::TextStart,
            CliEvent::Text("second.".to_string()),
        ];
        let mut out = String::new();
        let mut emitted_text = false;
        let mut need_separator = false;
        for ev in events {
            match ev {
                CliEvent::TextStart => {
                    if emitted_text {
                        need_separator = true;
                    }
                }
                CliEvent::Text(text) => {
                    if !text.is_empty() {
                        if need_separator {
                            out.push_str("\n\n[[thought:2]]\n\n");
                            need_separator = false;
                        }
                        out.push_str(&text);
                        emitted_text = true;
                    }
                }
                CliEvent::ToolUse | CliEvent::Error(_) => {}
            }
        }
        assert_eq!(out, "first.\n\n[[thought:2]]\n\nsecond.");
    }

    #[test]
    fn auto_detects_provider_from_model_name() {
        let arn = "arn:aws:bedrock:us-east-1:123:inference-profile/x";
        assert_eq!(pick(arn, "", false, true), Provider::Bedrock);
        assert_eq!(pick("gpt-4o", "", false, false), Provider::OpenAi);
        assert_eq!(pick("o3-mini", "", false, false), Provider::OpenAi);
        assert_eq!(pick("gemini-2.0-flash", "", false, false), Provider::Gemini);
        assert_eq!(pick("claude-sonnet-4-6", "", false, true), Provider::Anthropic);
        // claude-family / unknown with no key falls back to the CLI.
        assert_eq!(pick("claude-sonnet-4-6", "", false, false), Provider::ClaudeCli);
    }

    #[test]
    fn explicit_provider_overrides_detection() {
        // A GPT model name forced to the compatible/openrouter path, etc.
        assert_eq!(pick("gpt-4o", "openrouter", false, false), Provider::OpenAiCompatible);
        assert_eq!(pick("claude-sonnet-4-6", "openai", false, true), Provider::OpenAi);
        assert_eq!(pick("anything", "gemini", false, false), Provider::Gemini);
        // Unrecognized override is ignored → auto-detect.
        assert_eq!(pick("gpt-4o", "bogus", false, false), Provider::OpenAi);
    }

    #[test]
    fn a_custom_base_url_implies_openai_compatible() {
        assert_eq!(pick("some-local-model", "", true, false), Provider::OpenAiCompatible);
        // But an ARN still wins over a stray base URL.
        assert_eq!(pick("arn:aws:bedrock:...", "", true, false), Provider::Bedrock);
    }
}

#[cfg(test)]
mod sse_tests {
    use super::*;

    fn drain(input: &[u8]) -> (String, bool) {
        let mut buf = input.to_vec();
        let mut full = String::new();
        let mut extract = |data: &str| Some(data.to_string());
        let mut on = |_: StreamUpdate| {};
        let done = sse_drain_lines(&mut buf, &mut full, &mut extract, &mut on);
        (full, done)
    }

    #[test]
    fn drains_complete_lines_and_stops_on_done() {
        let (full, done) = drain(b"data: a\ndata: b\ndata: [DONE]\ndata: c\n");
        assert_eq!(full, "ab");
        assert!(done);
    }

    #[test]
    fn trailing_partial_line_is_recovered_with_pushed_newline() {
        // Mirrors consume_sse's end-of-stream path: a final line with no \n is
        // left in buf by the first drain, then recovered after pushing one.
        let mut buf = b"data: a\ndata: tail".to_vec();
        let mut full = String::new();
        let mut extract = |data: &str| Some(data.to_string());
        let mut on = |_: StreamUpdate| {};
        assert!(!sse_drain_lines(&mut buf, &mut full, &mut extract, &mut on));
        assert_eq!(full, "a");
        assert_eq!(buf, b"data: tail");
        buf.push(b'\n');
        sse_drain_lines(&mut buf, &mut full, &mut extract, &mut on);
        assert_eq!(full, "atail");
        assert!(buf.is_empty());
    }

    #[test]
    fn non_data_lines_and_crlf_are_tolerated() {
        let (full, done) = drain(b"event: ping\r\ndata: x\r\n\r\n");
        assert_eq!(full, "x");
        assert!(!done);
    }
}
