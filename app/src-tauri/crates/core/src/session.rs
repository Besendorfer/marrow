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
    /// Which PR lens (overview/files/commits, issue #170) this tab was on.
    /// `#[serde(default)]` so session.json files written before this field
    /// existed still deserialize instead of failing the whole restore.
    #[serde(default)]
    pub lens: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A session.json written before the `lens` field existed must still
    /// deserialize, defaulting the missing field to `None` rather than
    /// failing the whole restore (issue #170).
    #[test]
    fn deserializes_pre_lens_entry_with_lens_none() {
        let json = r#"{
            "pr_url": "https://github.com/o/r/pull/1",
            "selected_file": "src/lib.rs",
            "sidebar_view": "tree",
            "selected_comment_file": null
        }"#;
        let entry: SessionPrEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.lens, None);
    }

    #[test]
    fn round_trips_lens_field() {
        let entry = SessionPrEntry {
            pr_url: "https://github.com/o/r/pull/1".to_string(),
            selected_file: None,
            sidebar_view: None,
            lens: Some("files".to_string()),
            selected_comment_file: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let restored: SessionPrEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.lens, Some("files".to_string()));
    }
}
