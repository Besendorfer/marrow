use crate::config::app_config_dir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Per-PR set of dismissed AI highlights. `keys` are opaque, frontend-computed
/// stable identifiers (path + line range + comment hash) so a dismissal survives
/// re-fetches but re-surfaces if the underlying note changes.
///
/// Invariant: `keys` remains the authoritative hidden-set — every key in
/// `resolutions` must also be in `keys`; `resolutions` is best-effort metadata
/// layered on top (why/how a note was resolved). Old files (keys only) load
/// fine with empty resolutions; old app versions reading a new file ignore the
/// unknown `resolutions` field via ordinary serde behavior (no
/// `deny_unknown_fields` here, intentionally).
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct DismissedHighlights {
    pub keys: Vec<String>,
    /// key → how/why it was resolved. Absent for pre-resolution-state dismissals.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub resolutions: HashMap<String, NoteResolution>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NoteResolution {
    /// "fixed" | "intentional" | "noise". Defaulted so one malformed entry
    /// can't fail the whole file's parse and resurface every dismissed note
    /// (the UI renders an empty state as a plain "Dismissed").
    #[serde(default)]
    pub state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// ISO-8601; set by the frontend (core stays clock-free here).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub at: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_format_json_loads() {
        // Pre-resolution-state files only ever had `keys`.
        let json = r#"{"keys":["a.rs:1-2:abc","b.rs:3-4:def"]}"#;
        let parsed: DismissedHighlights = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.keys, vec!["a.rs:1-2:abc", "b.rs:3-4:def"]);
        assert!(parsed.resolutions.is_empty());
    }

    #[test]
    fn new_format_round_trips() {
        let mut resolutions = HashMap::new();
        resolutions.insert(
            "a.rs:1-2:abc".to_string(),
            NoteResolution {
                state: "fixed".to_string(),
                reason: "addressed in follow-up commit".to_string(),
                at: "2026-07-21T00:00:00.000Z".to_string(),
            },
        );
        let original = DismissedHighlights {
            keys: vec!["a.rs:1-2:abc".to_string()],
            resolutions,
        };

        let json = serde_json::to_string(&original).unwrap();
        let parsed: DismissedHighlights = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.keys, original.keys);
        assert_eq!(parsed.resolutions.len(), 1);
        let res = &parsed.resolutions["a.rs:1-2:abc"];
        assert_eq!(res.state, "fixed");
        assert_eq!(res.reason, "addressed in follow-up commit");
        assert_eq!(res.at, "2026-07-21T00:00:00.000Z");
    }

    #[test]
    fn resolution_without_matching_key_is_tolerated_on_load() {
        // `resolutions` is best-effort metadata; a stale/orphaned entry (its key
        // no longer in `keys`) must not fail to parse.
        let json = r#"{
            "keys": ["a.rs:1-2:abc"],
            "resolutions": {
                "b.rs:3-4:def": { "state": "noise", "reason": "", "at": "" }
            }
        }"#;
        let parsed: DismissedHighlights = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.keys, vec!["a.rs:1-2:abc"]);
        assert_eq!(parsed.resolutions.len(), 1);
        assert!(parsed.resolutions.contains_key("b.rs:3-4:def"));
    }
}
