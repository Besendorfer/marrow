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
    viewed_dir().join(format!("{}_{}_{}.json", crate::state_io::sanitize_key(owner), crate::state_io::sanitize_key(repo), pr_number))
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
    let path = viewed_path(owner, repo, pr_number);
    let json =
        serde_json::to_string_pretty(state).map_err(|e| format!("Failed to serialize: {}", e))?;
    crate::state_io::write_atomic(&path, json.as_bytes())
        .map_err(|e| format!("Failed to write viewed state: {}", e))?;

    Ok(())
}
