use crate::bedrock::BedrockClient;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

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

/// Which backend a (model, api-key) pair resolves to. Pure decision, separated
/// from construction so it's testable without hitting AWS/network.
#[derive(Debug, PartialEq, Eq)]
pub enum BackendChoice {
    Bedrock,
    Anthropic,
    ClaudeCli,
}

impl BackendChoice {
    /// Human-readable label for `marrow settings`.
    pub fn label(&self) -> &'static str {
        match self {
            BackendChoice::Bedrock => "bedrock",
            BackendChoice::Anthropic => "anthropic-api",
            BackendChoice::ClaudeCli => "claude-cli",
        }
    }
}

/// Decide the backend: an `arn:` model → Bedrock; otherwise an Anthropic API
/// key (config or env) → the direct Anthropic API; otherwise the `claude` CLI.
pub fn choose_backend(model: &str, anthropic_key: Option<&str>) -> BackendChoice {
    if model.starts_with("arn:") {
        BackendChoice::Bedrock
    } else if anthropic_key.is_some_and(|k| !k.is_empty()) {
        BackendChoice::Anthropic
    } else {
        BackendChoice::ClaudeCli
    }
}

/// AI backend that dispatches to AWS Bedrock (ARN model IDs), the Anthropic API
/// (a model name + API key), or the `claude` CLI (a model name, no key).
pub enum AiBackend {
    Bedrock {
        client: BedrockClient,
        model_arn: String,
    },
    Anthropic {
        api_key: String,
        model: String,
    },
    ClaudeCli {
        model: String,
    },
}

impl AiBackend {
    /// Create a backend from the model string and an optional Anthropic API key.
    /// See [`choose_backend`] for the dispatch rule.
    pub async fn new(
        model: &str,
        aws_profile: &str,
        anthropic_key: Option<&str>,
    ) -> Result<Self, String> {
        match choose_backend(model, anthropic_key) {
            BackendChoice::Bedrock => {
                let region = crate::bedrock::region_from_arn(model)?;
                let client = BedrockClient::new(&region, aws_profile).await?;
                Ok(AiBackend::Bedrock { client, model_arn: model.to_string() })
            }
            BackendChoice::Anthropic => Ok(AiBackend::Anthropic {
                api_key: anthropic_key.unwrap_or_default().to_string(),
                model: model.to_string(),
            }),
            BackendChoice::ClaudeCli => Ok(AiBackend::ClaudeCli { model: model.to_string() }),
        }
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

    #[test]
    fn arn_model_picks_bedrock() {
        let arn = "arn:aws:bedrock:us-east-1:123:inference-profile/x";
        assert_eq!(choose_backend(arn, None), BackendChoice::Bedrock);
        // An ARN takes precedence even if an API key is present.
        assert_eq!(choose_backend(arn, Some("sk-ant-xxx")), BackendChoice::Bedrock);
    }

    #[test]
    fn model_name_with_key_picks_anthropic() {
        assert_eq!(choose_backend("claude-sonnet-4-6", Some("sk-ant-xxx")), BackendChoice::Anthropic);
    }

    #[test]
    fn model_name_without_key_falls_back_to_cli() {
        assert_eq!(choose_backend("claude-sonnet-4-6", None), BackendChoice::ClaudeCli);
        // An empty key string is treated as "not set".
        assert_eq!(choose_backend("claude-sonnet-4-6", Some("")), BackendChoice::ClaudeCli);
    }
}
