use crate::config::app_config_dir;
use crate::dismissed_highlights::NoteResolution;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Per-PR set of resolved coverage-digest items (uncovered/partial
/// requirements, orphan tests — issue #179's "spec" rows). `keys` are opaque,
/// frontend-computed stable identifiers (see `resolveKey` in
/// app/src/components/digest.ts) — core never computes or interprets them.
/// Mirrors `DismissedHighlights`'s shape/conventions exactly.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ResolvedSpecs {
    #[serde(default)]
    pub keys: Vec<String>,
    /// key → how/why it was resolved.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub resolutions: HashMap<String, NoteResolution>,
}

fn resolved_specs_dir() -> PathBuf {
    app_config_dir().join("resolved_specs")
}

fn resolved_specs_path(owner: &str, repo: &str, pr_number: u64) -> PathBuf {
    resolved_specs_dir().join(format!("{}_{}_{}.json", crate::state_io::sanitize_key(owner), crate::state_io::sanitize_key(repo), pr_number))
}

pub fn load_resolved_specs(owner: &str, repo: &str, pr_number: u64) -> Option<ResolvedSpecs> {
    let path = resolved_specs_path(owner, repo, pr_number);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save_resolved_specs(
    owner: &str,
    repo: &str,
    pr_number: u64,
    state: &ResolvedSpecs,
) -> Result<(), String> {
    let path = resolved_specs_path(owner, repo, pr_number);
    let json =
        serde_json::to_string_pretty(state).map_err(|e| format!("Failed to serialize: {}", e))?;
    crate::state_io::write_atomic(&path, json.as_bytes())
        .map_err(|e| format!("Failed to write resolved specs state: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_without_resolutions() {
        let original = ResolvedSpecs {
            keys: vec!["spec:abc".to_string(), "orphan:def".to_string()],
            resolutions: HashMap::new(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: ResolvedSpecs = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.keys, original.keys);
        assert!(parsed.resolutions.is_empty());
    }

    #[test]
    fn round_trips_with_resolutions() {
        let mut resolutions = HashMap::new();
        resolutions.insert(
            "spec:abc".to_string(),
            NoteResolution {
                state: "addressed".to_string(),
                reason: "".to_string(),
                at: "2026-08-17T00:00:00.000Z".to_string(),
            },
        );
        let original = ResolvedSpecs {
            keys: vec!["spec:abc".to_string()],
            resolutions,
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: ResolvedSpecs = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.keys, original.keys);
        assert_eq!(parsed.resolutions.len(), 1);
        let res = &parsed.resolutions["spec:abc"];
        assert_eq!(res.state, "addressed");
        assert_eq!(res.at, "2026-08-17T00:00:00.000Z");
    }
}
