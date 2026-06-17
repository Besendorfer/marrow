use crate::types::Settings;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// The directory Marrow stores its config, cache, session, and viewed-state in.
///
/// - Linux / macOS: `$XDG_CONFIG_HOME/marrow`, else `~/.config/marrow`
/// - Windows: `%APPDATA%\marrow`
///
/// On first access, if this dir doesn't exist yet but the pre-rename
/// `~/.config/relevant-reviews/` dir does, its contents are copied over so
/// tokens/settings survive the rename. The old dir is left intact as a fallback.
pub fn app_config_dir() -> PathBuf {
    let dir = config_base().join("marrow");
    migrate_legacy_config(legacy_config_dir(), &dir);
    dir
}

pub fn config_path() -> PathBuf {
    app_config_dir().join("config")
}

/// The platform's base config directory (the parent of the `marrow` dir).
fn config_base() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(appdata) = env::var("APPDATA") {
            if !appdata.is_empty() {
                return PathBuf::from(appdata);
            }
        }
        return PathBuf::from(env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string()))
            .join("AppData")
            .join("Roaming");
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg);
            }
        }
        let home = env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home).join(".config")
    }
}

/// The pre-rename config dir (always `~/.config/relevant-reviews/`, as the old
/// code hardcoded it). `None` if `HOME` is unset.
fn legacy_config_dir() -> Option<PathBuf> {
    let home = env::var("HOME").ok().filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".config").join("relevant-reviews"))
}

/// One-time, non-destructive migration: if `new` doesn't exist yet but `old`
/// does, copy `old`'s contents into `new`, leaving `old` untouched. Best-effort
/// — a copy failure must not break config access (we just fall through to a
/// fresh config dir).
fn migrate_legacy_config(old: Option<PathBuf>, new: &Path) {
    if new.exists() {
        return;
    }
    let Some(old) = old else { return };
    if !old.exists() || old.as_path() == new {
        return;
    }
    let _ = copy_dir_all(&old, new);
}

/// Recursively copy a directory's contents (files keep their permissions, so a
/// `0600` config stays `0600`).
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn default_settings() -> Settings {
    Settings {
        model: String::new(),
        github_token: String::new(),
        aws_profile: String::new(),
        filter_older: true,
        filter_team: true,
        view_mode: "split".to_string(),
        show_hunk_significance: true,
        show_ai_notes: true,
        hunk_filter: "all".to_string(),
    }
}

pub fn load_settings() -> Settings {
    let path = config_path();
    if !path.exists() {
        return default_settings();
    }

    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return default_settings(),
    };

    let mut model = String::new();
    let mut github_token = String::new();
    let mut aws_profile = String::new();
    let mut filter_older = true;
    let mut filter_team = true;
    let mut view_mode = "split".to_string();
    let mut show_hunk_significance = true;
    let mut show_ai_notes = true;
    let mut hunk_filter = "all".to_string();

    for line in content.lines() {
        if let Some(val) = line.strip_prefix("model=") {
            model = val.to_string();
        } else if let Some(val) = line.strip_prefix("github_token=") {
            github_token = val.to_string();
        } else if let Some(val) = line.strip_prefix("aws_profile=") {
            aws_profile = val.to_string();
        } else if let Some(val) = line.strip_prefix("filter_older=") {
            filter_older = val == "true";
        } else if let Some(val) = line.strip_prefix("filter_team=") {
            filter_team = val == "true";
        } else if let Some(val) = line.strip_prefix("view_mode=") {
            view_mode = val.to_string();
        } else if let Some(val) = line.strip_prefix("show_hunk_significance=") {
            show_hunk_significance = val == "true";
        } else if let Some(val) = line.strip_prefix("show_ai_notes=") {
            show_ai_notes = val == "true";
        } else if let Some(val) = line.strip_prefix("hunk_filter=") {
            hunk_filter = val.to_string();
        }
    }

    Settings {
        model,
        github_token,
        aws_profile,
        filter_older,
        filter_team,
        view_mode,
        show_hunk_significance,
        show_ai_notes,
        hunk_filter,
    }
}

pub fn save_settings_to_disk(settings: &Settings) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }

    let mut content = format!("model={}\n", settings.model);
    if !settings.github_token.is_empty() {
        content.push_str(&format!("github_token={}\n", settings.github_token));
    }
    if !settings.aws_profile.is_empty() {
        content.push_str(&format!("aws_profile={}\n", settings.aws_profile));
    }
    content.push_str(&format!("filter_older={}\n", settings.filter_older));
    content.push_str(&format!("filter_team={}\n", settings.filter_team));
    content.push_str(&format!("view_mode={}\n", settings.view_mode));
    content.push_str(&format!("show_hunk_significance={}\n", settings.show_hunk_significance));
    content.push_str(&format!("show_ai_notes={}\n", settings.show_ai_notes));
    content.push_str(&format!("hunk_filter={}\n", settings.hunk_filter));

    fs::write(&path, content).map_err(|e| format!("Failed to save settings: {}", e))?;

    // Set restrictive permissions since we're storing a token
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(&path, perms);
    }

    Ok(())
}

/// Resolve a GitHub token: config file > GH_TOKEN env > GITHUB_TOKEN env.
/// Returns None if no token is configured (fine for public repos).
pub fn resolve_github_token(settings: &Settings) -> Option<String> {
    if !settings.github_token.is_empty() {
        return Some(settings.github_token.clone());
    }

    if let Ok(token) = env::var("GH_TOKEN") {
        if !token.is_empty() {
            return Some(token);
        }
    }

    if let Ok(token) = env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            return Some(token);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A unique, never-reused temp path (no rand/time dependency).
    fn unique_tmp(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!("marrow-cfgtest-{}-{}-{}", tag, std::process::id(), n))
    }

    #[test]
    fn migrate_copies_contents_when_new_absent() {
        let old = unique_tmp("old");
        let new = unique_tmp("new");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("config"), "github_token=abc\n").unwrap();
        fs::create_dir_all(old.join("cache")).unwrap();
        fs::write(old.join("cache").join("x.json"), "{}").unwrap();

        migrate_legacy_config(Some(old.clone()), &new);

        assert_eq!(fs::read_to_string(new.join("config")).unwrap(), "github_token=abc\n");
        assert!(new.join("cache").join("x.json").exists(), "nested files copied");
        assert!(old.join("config").exists(), "old dir left intact (non-destructive)");

        let _ = fs::remove_dir_all(&old);
        let _ = fs::remove_dir_all(&new);
    }

    #[test]
    fn migrate_is_noop_when_new_already_exists() {
        let old = unique_tmp("old");
        let new = unique_tmp("new");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("config"), "old\n").unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("config"), "new\n").unwrap();

        migrate_legacy_config(Some(old.clone()), &new);
        assert_eq!(
            fs::read_to_string(new.join("config")).unwrap(),
            "new\n",
            "an existing config dir is never overwritten"
        );

        let _ = fs::remove_dir_all(&old);
        let _ = fs::remove_dir_all(&new);
    }

    #[test]
    fn migrate_is_noop_without_a_legacy_dir() {
        let new = unique_tmp("new");
        migrate_legacy_config(None, &new);
        assert!(!new.exists(), "nothing is created when HOME/legacy dir is absent");
        migrate_legacy_config(Some(unique_tmp("missing")), &new);
        assert!(!new.exists(), "nothing is created when the legacy dir doesn't exist");
    }
}
