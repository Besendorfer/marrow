use crate::config::app_config_dir;
use std::fs;
use std::path::PathBuf;

fn dismiss_dir() -> PathBuf {
    app_config_dir().join("checks_dismissed")
}

fn dismiss_path(owner: &str, repo: &str, pr_number: u64) -> PathBuf {
    dismiss_dir().join(format!("{}_{}_{}", owner, repo, pr_number))
}

pub fn is_dismissed(owner: &str, repo: &str, pr_number: u64) -> bool {
    dismiss_path(owner, repo, pr_number).exists()
}

pub fn set_dismissed(owner: &str, repo: &str, pr_number: u64) -> Result<(), String> {
    let dir = dismiss_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create dismiss dir: {}", e))?;
    let path = dismiss_path(owner, repo, pr_number);
    fs::write(&path, "").map_err(|e| format!("Failed to write dismiss marker: {}", e))?;
    Ok(())
}
