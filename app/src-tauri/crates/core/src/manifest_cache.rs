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
    cache_dir().join(format!("{}_{}_{}.json", owner, repo, pr_number))
}

fn meta_path(owner: &str, repo: &str, pr_number: u64) -> PathBuf {
    cache_dir().join(format!("{}_{}_{}.meta.json", owner, repo, pr_number))
}

/// Per-PR advisory lock serializing the manifest + metadata pair write.
fn lock_path(owner: &str, repo: &str, pr_number: u64) -> PathBuf {
    cache_dir().join(format!("{}_{}_{}.lock", owner, repo, pr_number))
}

pub fn delete_cached_manifest(owner: &str, repo: &str, pr_number: u64) {
    let _ = fs::remove_file(cache_path(owner, repo, pr_number));
    let _ = fs::remove_file(meta_path(owner, repo, pr_number));
    let _ = fs::remove_file(lock_path(owner, repo, pr_number));
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
    crate::state_io::with_lock(&lock, || -> Result<(), String> {
        crate::state_io::write_atomic(&path, json.as_bytes())
            .map_err(|e| format!("Failed to write cached manifest: {}", e))?;
        crate::state_io::write_atomic(&mpath, meta_json.as_bytes())
            .map_err(|e| format!("Failed to write metadata: {}", e))
    })??;

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
    let dir = cache_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut results: Vec<CachedPrInfo> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if !name.ends_with(".meta.json") {
                return None;
            }
            let content = fs::read_to_string(&path).ok()?;
            serde_json::from_str(&content).ok()
        })
        .collect();

    // Fall back to full manifest parsing for entries without metadata files
    if results.is_empty() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        results = entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                let name = path.file_name()?.to_str()?;
                if !name.ends_with(".json") || name.ends_with(".meta.json") {
                    return None;
                }
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
            })
            .collect();
    }

    results.sort_by(|a, b| b.cached_at.cmp(&a.cached_at));
    results
}
