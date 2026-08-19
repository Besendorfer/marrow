use crate::config::app_config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// User-provided requirements text for a PR (issue #179 phase 2) — an
/// authoritative override/supplement to the PR description when the
/// description itself states no real requirements (or the reviewer wants to
/// judge coverage against something more precise). Mirrors
/// `resolved_specs.rs`'s shape/conventions exactly.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PrRequirements {
    #[serde(default)]
    pub text: String,
}

fn pr_requirements_dir() -> PathBuf {
    app_config_dir().join("pr_requirements")
}

/// Restrict a path component to a safe charset so a hostile `owner`/`repo` from
/// the IPC boundary can't escape the cache dir. A no-op for real GitHub names.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

fn pr_requirements_path(owner: &str, repo: &str, pr_number: u64) -> PathBuf {
    pr_requirements_dir().join(format!("{}_{}_{}.json", sanitize(owner), sanitize(repo), pr_number))
}

pub fn load_pr_requirements(owner: &str, repo: &str, pr_number: u64) -> Option<PrRequirements> {
    let path = pr_requirements_path(owner, repo, pr_number);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save_pr_requirements(
    owner: &str,
    repo: &str,
    pr_number: u64,
    state: &PrRequirements,
) -> Result<(), String> {
    let dir = pr_requirements_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create pr_requirements dir: {}", e))?;
    let path = pr_requirements_path(owner, repo, pr_number);
    let json =
        serde_json::to_string_pretty(state).map_err(|e| format!("Failed to serialize: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("Failed to write pr requirements state: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(&path, perms);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let original = PrRequirements {
            text: "The endpoint must return 404 for unknown ids.".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: PrRequirements = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.text, original.text);
    }
}
