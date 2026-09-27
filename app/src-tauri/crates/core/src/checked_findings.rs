use crate::config::app_config_dir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Per-PR "Looks fine" marks on review findings (issue #238). `entries` keys
/// are opaque, frontend-computed finding keys (see `findingKey`/`buildFindings`
/// in app/src/review/findings.ts) — core never computes or interprets them.
///
/// Deliberately separate from `DismissedHighlights`: dismissed keys are the
/// hidden-in-diff set and feed the AI's prior-notes context, while a checked
/// finding stays visible and must reopen once its code changes. The frontend
/// does that by comparing `lines_hash` against the finding's current lines.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct CheckedFindings {
    #[serde(default)]
    pub entries: HashMap<String, CheckedEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CheckedEntry {
    /// Hash of the lines the finding sat on when it was checked. Defaulted so
    /// one malformed entry can't fail the whole file's parse (an empty hash
    /// simply never matches, so the finding reads as open).
    #[serde(default)]
    pub lines_hash: String,
    /// ISO-8601; set by the frontend (core stays clock-free here).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub at: String,
}

fn checked_findings_dir() -> PathBuf {
    app_config_dir().join("checked_findings")
}

fn checked_findings_path(owner: &str, repo: &str, pr_number: u64) -> PathBuf {
    checked_findings_dir().join(format!(
        "{}_{}_{}.json",
        crate::state_io::sanitize_key(owner),
        crate::state_io::sanitize_key(repo),
        pr_number
    ))
}

pub fn load_checked_findings(owner: &str, repo: &str, pr_number: u64) -> Option<CheckedFindings> {
    let path = checked_findings_path(owner, repo, pr_number);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save_checked_findings(
    owner: &str,
    repo: &str,
    pr_number: u64,
    state: &CheckedFindings,
) -> Result<(), String> {
    let path = checked_findings_path(owner, repo, pr_number);
    let json =
        serde_json::to_string_pretty(state).map_err(|e| format!("Failed to serialize: {}", e))?;
    crate::state_io::write_atomic(&path, json.as_bytes())
        .map_err(|e| format!("Failed to write checked findings state: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_entries() {
        let mut entries = HashMap::new();
        entries.insert(
            "risk:src/a.rs:10:abc".to_string(),
            CheckedEntry { lines_hash: "h1".to_string(), at: "2026-09-27T00:00:00.000Z".to_string() },
        );
        let original = CheckedFindings { entries };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: CheckedFindings = serde_json::from_str(&json).unwrap();
        let e = &parsed.entries["risk:src/a.rs:10:abc"];
        assert_eq!(e.lines_hash, "h1");
        assert_eq!(e.at, "2026-09-27T00:00:00.000Z");
    }

    #[test]
    fn tolerates_missing_and_malformed_fields() {
        // Empty file object → no entries; an entry missing lines_hash still
        // parses (defaults to "", which never matches a real hash).
        let parsed: CheckedFindings = serde_json::from_str("{}").unwrap();
        assert!(parsed.entries.is_empty());
        let parsed: CheckedFindings = serde_json::from_str(r#"{"entries":{"k":{}}}"#).unwrap();
        assert_eq!(parsed.entries["k"].lines_hash, "");
    }
}
