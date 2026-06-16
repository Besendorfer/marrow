use crate::config::app_config_dir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ViewedFileState {
    pub files: HashMap<String, String>, // path -> diff_hash
}

fn viewed_dir() -> PathBuf {
    app_config_dir().join("viewed")
}

fn viewed_path(owner: &str, repo: &str, pr_number: u64) -> PathBuf {
    viewed_dir().join(format!("{}_{}_{}.json", owner, repo, pr_number))
}

pub fn load_viewed_state(owner: &str, repo: &str, pr_number: u64) -> Option<ViewedFileState> {
    let path = viewed_path(owner, repo, pr_number);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save_viewed_state(
    owner: &str,
    repo: &str,
    pr_number: u64,
    state: &ViewedFileState,
) -> Result<(), String> {
    let dir = viewed_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create viewed dir: {}", e))?;
    let path = viewed_path(owner, repo, pr_number);
    let json =
        serde_json::to_string_pretty(state).map_err(|e| format!("Failed to serialize: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("Failed to write viewed state: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(&path, perms);
    }

    Ok(())
}
