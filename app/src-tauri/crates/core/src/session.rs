use crate::config::app_config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

fn session_path() -> PathBuf {
    app_config_dir().join("session.json")
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionPrEntry {
    pub pr_url: String,
    pub selected_file: Option<String>,
    pub sidebar_view: Option<String>,
    pub selected_comment_file: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionState {
    pub open_prs: Vec<SessionPrEntry>,
    pub active_pr: Option<String>,
}

pub fn load_session_state() -> Option<SessionState> {
    let path = session_path();
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save_session_state(state: &SessionState) -> Result<(), String> {
    let path = session_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }
    let json =
        serde_json::to_string(state).map_err(|e| format!("Failed to serialize session: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("Failed to write session: {}", e))?;
    Ok(())
}
