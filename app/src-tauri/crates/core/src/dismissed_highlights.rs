use crate::config::app_config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Per-PR set of dismissed AI highlights. `keys` are opaque, frontend-computed
/// stable identifiers (path + line range + comment hash) so a dismissal survives
/// re-fetches but re-surfaces if the underlying note changes.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct DismissedHighlights {
    pub keys: Vec<String>,
}

fn dismissed_dir() -> PathBuf {
    app_config_dir().join("dismissed")
}

/// Restrict a path component to a safe charset so a hostile `owner`/`repo` from
/// the IPC boundary can't escape the cache dir. A no-op for real GitHub names.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

fn dismissed_path(owner: &str, repo: &str, pr_number: u64) -> PathBuf {
    dismissed_dir().join(format!("{}_{}_{}.json", sanitize(owner), sanitize(repo), pr_number))
}

pub fn load_dismissed(owner: &str, repo: &str, pr_number: u64) -> Option<DismissedHighlights> {
    let path = dismissed_path(owner, repo, pr_number);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save_dismissed(
    owner: &str,
    repo: &str,
    pr_number: u64,
    state: &DismissedHighlights,
) -> Result<(), String> {
    let dir = dismissed_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create dismissed dir: {}", e))?;
    let path = dismissed_path(owner, repo, pr_number);
    let json =
        serde_json::to_string_pretty(state).map_err(|e| format!("Failed to serialize: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("Failed to write dismissed state: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(&path, perms);
    }

    Ok(())
}
