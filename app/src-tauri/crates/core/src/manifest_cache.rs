use crate::config::app_config_dir;
use crate::pr_parser::parse_pr_ref;
use crate::types::ReviewManifest;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

fn cache_dir() -> PathBuf {
    app_config_dir().join("manifests")
}

fn cache_path(owner: &str, repo: &str, pr_number: u64) -> PathBuf {
    cache_dir().join(format!("{}_{}_{}.json", crate::state_io::sanitize_key(owner), crate::state_io::sanitize_key(repo), pr_number))
}

fn meta_path(owner: &str, repo: &str, pr_number: u64) -> PathBuf {
    cache_dir().join(format!("{}_{}_{}.meta.json", crate::state_io::sanitize_key(owner), crate::state_io::sanitize_key(repo), pr_number))
}

/// Per-PR advisory lock serializing the manifest + metadata pair write.
fn lock_path(owner: &str, repo: &str, pr_number: u64) -> PathBuf {
    cache_dir().join(format!("{}_{}_{}.lock", crate::state_io::sanitize_key(owner), crate::state_io::sanitize_key(repo), pr_number))
}

pub fn delete_cached_manifest(owner: &str, repo: &str, pr_number: u64) {
    // Delete under the pair lock so an in-flight save can't interleave. The
    // lock FILE itself is never unlinked: removing a lock another process
    // may hold makes the next locker open a fresh inode, silently breaking
    // mutual exclusion. A stray empty .lock is harmless.
    let _ = crate::state_io::with_lock(&lock_path(owner, repo, pr_number), || {
        let _ = fs::remove_file(cache_path(owner, repo, pr_number));
        let _ = fs::remove_file(meta_path(owner, repo, pr_number));
        Ok(())
    });
}

pub fn load_cached_manifest(owner: &str, repo: &str, pr_number: u64) -> Option<ReviewManifest> {
    let path = cache_path(owner, repo, pr_number);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save_cached_manifest(
    owner: &str,
    repo: &str,
    pr_number: u64,
    manifest: &ReviewManifest,
) -> Result<(), String> {
    let path = cache_path(owner, repo, pr_number);
    let json = serde_json::to_string(manifest)
        .map_err(|e| format!("Failed to serialize manifest: {}", e))?;

    let meta = CachedPrInfo {
        owner: owner.to_string(),
        repo: repo.to_string(),
        pr_number: manifest.pr_number,
        pr_title: manifest.pr_title.clone(),
        pr_url: manifest.pr_url.clone(),
        head_sha: manifest.head_sha.clone(),
        file_count: manifest.files.len(),
        cached_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    let meta_json = serde_json::to_string(&meta)
        .map_err(|e| format!("Failed to serialize metadata: {}", e))?;
    let mpath = meta_path(owner, repo, pr_number);

    // The manifest and its metadata sidecar must correspond — two concurrent
    // fetch completions could otherwise pair A's manifest with B's metadata.
    // The per-PR lock serializes the pair; each write is still atomic on its
    // own, so even unlocked writers (none today) can't tear a single file.
    //
    // Write ORDER is deliberate: manifest (authoritative content) first,
    // metadata (queue-display cosmetics) second. If the meta write fails, or
    // an unlocked reader lands between the two writes, the worst case is a
    // correct new manifest labeled by a stale sidecar — self-healed by the
    // next save. The reverse order could advertise an analysis that isn't
    // there.
    let lock = lock_path(owner, repo, pr_number);
    crate::state_io::with_lock(&lock, || {
        crate::state_io::write_atomic(&path, json.as_bytes())
            .map_err(|e| format!("Failed to write cached manifest: {}", e))?;
        crate::state_io::write_atomic(&mpath, meta_json.as_bytes())
            .map_err(|e| format!("Failed to write metadata: {}", e))
    })?;

    Ok(())
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CachedPrInfo {
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub pr_title: String,
    pub pr_url: String,
    pub head_sha: String,
    pub file_count: usize,
    pub cached_at: String,
}

pub fn list_cached_manifests() -> Vec<CachedPrInfo> {
    list_cached_in(&cache_dir())
}

/// Directory-injectable core of `list_cached_manifests` (testable without
/// touching the real config dir).
fn list_cached_in(dir: &std::path::Path) -> Vec<CachedPrInfo> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    // ONE merged pass (issue #214): sidecar-backed entries are cheap and
    // preferred; a manifest WITHOUT a sidecar (cached before the .meta.json
    // era, or whose sidecar write failed) is parsed in full instead of being
    // hidden. The old code fell back only when the sidecar scan found
    // nothing at all — a single new-style cache hid every old entry.
    let mut results: Vec<CachedPrInfo> = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if name.ends_with(".meta.json") {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(info) = serde_json::from_str::<CachedPrInfo>(&content) {
                    results.push(info);
                }
            }
        } else if name.ends_with(".json") {
            // Manifest file: only parsed when its sidecar is absent.
            if dir.join(format!("{}.meta.json", name.trim_end_matches(".json"))).exists() {
                continue;
            }
            let info = (|| {
                let content = fs::read_to_string(&path).ok()?;
                let manifest: ReviewManifest = serde_json::from_str(&content).ok()?;
                let parsed = parse_pr_ref(&manifest.pr_url).ok()?;
                let mtime = entry.metadata().ok()?.modified().ok()?;
                let cached_at: DateTime<Utc> = mtime.into();
                Some(CachedPrInfo {
                    owner: parsed.owner,
                    repo: parsed.repo,
                    pr_number: manifest.pr_number,
                    pr_title: manifest.pr_title,
                    pr_url: manifest.pr_url,
                    head_sha: manifest.head_sha,
                    file_count: manifest.files.len(),
                    cached_at: cached_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                })
            })();
            if let Some(info) = info {
                results.push(info);
            }
        }
    }

    results.sort_by(|a, b| b.cached_at.cmp(&a.cached_at));
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ReviewManifest;

    fn manifest(pr: u64, title: &str) -> ReviewManifest {
        ReviewManifest {
            pr_title: title.to_string(),
            pr_url: format!("https://github.com/o/r/pull/{pr}"),
            pr_number: pr,
            base_ref: "main".into(),
            head_ref: "b".into(),
            base_sha: "a".into(),
            head_sha: "h".into(),
            author: String::new(),
            draft: false,
            summary: String::new(),
            change_groups: Vec::new(),
            triage: None,
            requirements_coverage: None,
            body: String::new(),
            commits: Vec::new(),
            passes: Vec::new(),
            analysis_truncated: false,
            truncated_passes: Vec::new(),
            failed_passes: Vec::new(),
            analysis_fingerprint: None,
            files: Vec::new(),
        }
    }

    #[test]
    fn discovery_merges_sidecar_and_manifest_only_entries() {
        let dir = std::env::temp_dir().join(format!("marrow-disc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // New-style: manifest + sidecar (sidecar is the source of truth).
        fs::write(dir.join("o_r_1.json"), serde_json::to_string(&manifest(1, "new")).unwrap()).unwrap();
        let meta = CachedPrInfo {
            owner: "o".into(), repo: "r".into(), pr_number: 1, pr_title: "new".into(),
            pr_url: "https://github.com/o/r/pull/1".into(), head_sha: "h".into(),
            file_count: 0, cached_at: "2026-08-30T00:00:00Z".into(),
        };
        fs::write(dir.join("o_r_1.meta.json"), serde_json::to_string(&meta).unwrap()).unwrap();
        // Old-style: manifest only, no sidecar — the old all-or-nothing
        // fallback HID this entry whenever any sidecar existed.
        fs::write(dir.join("o_r_2.json"), serde_json::to_string(&manifest(2, "old")).unwrap()).unwrap();

        let listed = list_cached_in(&dir);
        let mut prs: Vec<u64> = listed.iter().map(|i| i.pr_number).collect();
        prs.sort();
        assert_eq!(prs, vec![1, 2], "old and new entries must MERGE, not hide");
        // No double-listing of the sidecar-backed PR.
        assert_eq!(listed.iter().filter(|i| i.pr_number == 1).count(), 1);

        let _ = fs::remove_dir_all(&dir);
    }
}
