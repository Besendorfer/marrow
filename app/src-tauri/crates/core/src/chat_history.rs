use crate::config::app_config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// One turn of the per-PR review chat. `role` is "user" or "assistant".
/// `file_path` records which file was in focus when a user message was sent
/// (None for whole-PR scope), purely for display/context — it does not affect
/// loading.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
}

/// Persisted conversation for a single PR. Mirrors the per-PR stores used for
/// dismissed highlights and viewed state.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct StoredChat {
    pub messages: Vec<StoredMessage>,
}

fn chat_dir() -> PathBuf {
    app_config_dir().join("chat")
}

/// Restrict a path component to a safe charset so a hostile `owner`/`repo` from
/// the IPC boundary can't escape the cache dir. A no-op for real GitHub names.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

fn chat_path(owner: &str, repo: &str, pr_number: u64) -> PathBuf {
    chat_dir().join(format!("{}_{}_{}.json", sanitize(owner), sanitize(repo), pr_number))
}

pub fn load_chat(owner: &str, repo: &str, pr_number: u64) -> Option<StoredChat> {
    let path = chat_path(owner, repo, pr_number);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save_chat(owner: &str, repo: &str, pr_number: u64, state: &StoredChat) -> Result<(), String> {
    let dir = chat_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create chat dir: {}", e))?;
    let path = chat_path(owner, repo, pr_number);
    let json =
        serde_json::to_string_pretty(state).map_err(|e| format!("Failed to serialize: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("Failed to write chat history: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(&path, perms);
    }

    Ok(())
}
