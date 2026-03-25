use crate::types::Settings;
use std::env;
use std::fs;
use std::path::PathBuf;

pub fn config_path() -> PathBuf {
    let home = env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("relevant-reviews")
        .join("config")
}

fn default_settings() -> Settings {
    Settings {
        model: String::new(),
        github_token: String::new(),
        aws_profile: String::new(),
        filter_older: true,
        filter_team: true,
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
        }
    }

    Settings {
        model,
        github_token,
        aws_profile,
        filter_older,
        filter_team,
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
/// Adding a test comment to see if it syncs to Relevant Reviews properly.
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
