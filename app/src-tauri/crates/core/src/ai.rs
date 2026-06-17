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
    let mut child = Command::new(resolve_claude_binary())
        .args(["--model", model, "--print"])
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
