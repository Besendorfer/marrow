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
    let dir = cache_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create cache dir: {}", e))?;

    let path = cache_path(owner, repo, pr_number);
    let json = serde_json::to_string(manifest)
        .map_err(|e| format!("Failed to serialize manifest: {}", e))?;
    fs::write(&path, &json).map_err(|e| format!("Failed to write cached manifest: {}", e))?;
    set_private_permissions(&path);

    // Write lightweight metadata sidecar
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
    fs::write(&mpath, meta_json).map_err(|e| format!("Failed to write metadata: {}", e))?;
    set_private_permissions(&mpath);

    Ok(())
}

fn set_private_permissions(_path: &PathBuf) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(_path, perms);
    }
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
