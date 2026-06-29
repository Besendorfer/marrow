//! User-defined "watches": saved GitHub searches that surface PRs the user
//! cares about — including repos/orgs where they are *not* a requested
//! reviewer. Stored as a JSON sidecar in the config dir (the flat key=value
//! `config` file can't represent a list of structured entries).

use crate::config::app_config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// A single saved GitHub search. `query` is raw GitHub search syntax, e.g.
/// `is:pr is:open repo:acme/web -is:draft`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Watch {
    pub id: String,
    pub label: String,
    pub query: String,
}

fn watches_path() -> PathBuf {
    app_config_dir().join("watches.json")
}

/// Load the saved watches. Returns an empty list if none are configured yet
/// (a missing or unparseable file is treated as "no watches").
pub fn load_watches() -> Vec<Watch> {
    let path = watches_path();
    let Ok(content) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

pub fn save_watches(watches: &[Watch]) -> Result<(), String> {
    let path = watches_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create config dir: {}", e))?;
    }
    let json = serde_json::to_string_pretty(watches)
        .map_err(|e| format!("Failed to serialize watches: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("Failed to write watches: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(&path, perms);
    }

    Ok(())
}
