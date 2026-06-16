use crate::types::Settings;
use std::env;
use std::fs;
use std::path::PathBuf;

pub fn app_config_dir() -> PathBuf {
    let home = env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("relevant-reviews")
}

pub fn config_path() -> PathBuf {
    app_config_dir().join("config")
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
