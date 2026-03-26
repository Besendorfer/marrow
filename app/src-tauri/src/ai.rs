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

/// AI backend that dispatches to either AWS Bedrock (for ARN model IDs) or the
/// `claude` CLI (for direct model names like `claude-sonnet-4-6`).
pub enum AiBackend {
    Bedrock {
        client: BedrockClient,
        model_arn: String,
    },
    ClaudeCli {
        model: String,
    },
}

impl AiBackend {
    /// Create a new AI backend based on the model string.
    /// If the model starts with "arn:", use Bedrock. Otherwise, use the claude CLI.
    pub async fn new(model: &str, aws_profile: &str) -> Result<Self, String> {
        if model.starts_with("arn:") {
            let region = crate::bedrock::region_from_arn(model)?;
            let client = BedrockClient::new(&region, aws_profile).await?;
            Ok(AiBackend::Bedrock {
                client,
                model_arn: model.to_string(),
            })
        } else {
            Ok(AiBackend::ClaudeCli {
                model: model.to_string(),
            })
        }
    }

    /// Send a prompt to the AI and return the text response.
    pub async fn invoke(&self, prompt: &str) -> Result<String, String> {
        match self {
            AiBackend::Bedrock { client, model_arn } => {
                client.invoke_model(model_arn, prompt).await
            }
            AiBackend::ClaudeCli { model } => {
                invoke_claude_cli(model, prompt).await
            }
        }
    }

}

async fn invoke_claude_cli(model: &str, prompt: &str) -> Result<String, String> {
    let mut child = Command::new("claude")
        .args(["--model", model, "--print"])
        .env("CLAUDECODE", "") // prevent recursive Claude Code invocation
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "Failed to run claude CLI: {}. Is the `claude` command installed and on your PATH?",
                e
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
