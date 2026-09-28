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

/// Whether the floating mini-player may show: the activity master switch is
/// on and its own auto-show toggle (✕ / ⧉) hasn't turned it off.
pub fn floating_mini_player_enabled(settings: &Settings) -> bool {
    settings.show_activity && settings.activity_mini_player
}

fn default_settings() -> Settings {
    Settings {
        model: String::new(),
        github_token: String::new(),
        aws_profile: String::new(),
        anthropic_api_key: String::new(),
        provider: String::new(),
        openai_api_key: String::new(),
        gemini_api_key: String::new(),
        typesafe_api_key: String::new(),
        openai_base_url: String::new(),
        filter_older: true,
        filter_team: true,
        view_mode: "split".to_string(),
        show_hunk_significance: true,
        show_ai_notes: true,
        hunk_filter: "all".to_string(),
        activity_per_watch_cap: 50,
        activity_mini_player: true,
        show_activity: false,
        show_approved_prs: false,
        show_draft_prs: true,
        setup_done: false,
        expand_all_hunks: false,
        classic_layout: false,
        local_repo_roots: Vec::new(),
    }
}

pub fn load_settings() -> Settings {
    let path = config_path();
    if !path.exists() {
        return default_settings();
    }

    match fs::read_to_string(&path) {
        Ok(content) => parse_settings(&content),
        Err(_) => default_settings(),
    }
}

/// Parse the hand-rolled `key=value` config format. Pure (no I/O) so the
/// format is testable without touching the user's real config file.
pub fn parse_settings(content: &str) -> Settings {
    let mut model = String::new();
    let mut github_token = String::new();
    let mut aws_profile = String::new();
    let mut anthropic_api_key = String::new();
    let mut provider = String::new();
    let mut openai_api_key = String::new();
    let mut gemini_api_key = String::new();
    let mut typesafe_api_key = String::new();
    let mut openai_base_url = String::new();
    let mut filter_older = true;
    let mut filter_team = true;
    let mut view_mode = "split".to_string();
    let mut show_hunk_significance = true;
    let mut show_ai_notes = true;
    let mut hunk_filter = "all".to_string();
    let mut activity_per_watch_cap = 50u64;
    let mut activity_mini_player = true;
    let mut show_activity = false;
    let mut show_approved_prs = false;
    let mut show_draft_prs = true;
    let mut setup_done = false;
    let mut expand_all_hunks = false;
    let mut classic_layout = false;
    // One `local_repo_root=` line per directory (paths may contain commas).
    let mut local_repo_roots: Vec<String> = Vec::new();

    for line in content.lines() {
        if let Some(val) = line.strip_prefix("model=") {
            model = val.to_string();
        } else if let Some(val) = line.strip_prefix("github_token=") {
            github_token = val.to_string();
        } else if let Some(val) = line.strip_prefix("aws_profile=") {
            aws_profile = val.to_string();
        } else if let Some(val) = line.strip_prefix("anthropic_api_key=") {
            anthropic_api_key = val.to_string();
        } else if let Some(val) = line.strip_prefix("provider=") {
            provider = val.to_string();
        } else if let Some(val) = line.strip_prefix("openai_api_key=") {
            openai_api_key = val.to_string();
        } else if let Some(val) = line.strip_prefix("gemini_api_key=") {
            gemini_api_key = val.to_string();
        } else if let Some(val) = line.strip_prefix("typesafe_api_key=") {
            typesafe_api_key = val.to_string();
        } else if let Some(val) = line.strip_prefix("vercel_ai_gateway_api_key=") {
            // The #249 draft briefly stored the Jev key under this name; read
            // it once so the next save writes it as typesafe_api_key.
            if typesafe_api_key.is_empty() {
                typesafe_api_key = val.to_string();
            }
        } else if let Some(val) = line.strip_prefix("openai_base_url=") {
            openai_base_url = val.to_string();
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
        } else if let Some(val) = line.strip_prefix("activity_per_watch_cap=") {
            if let Ok(n) = val.parse::<u64>() {
                activity_per_watch_cap = n;
            }
        } else if let Some(val) = line.strip_prefix("activity_mini_player=") {
            activity_mini_player = val == "true";
        } else if let Some(val) = line.strip_prefix("show_activity=") {
            show_activity = val == "true";
        } else if let Some(val) = line.strip_prefix("show_approved_prs=") {
            show_approved_prs = val == "true";
        } else if let Some(val) = line.strip_prefix("show_draft_prs=") {
            show_draft_prs = val == "true";
        } else if let Some(val) = line.strip_prefix("setup_done=") {
            setup_done = val == "true";
        } else if let Some(val) = line.strip_prefix("expand_all_hunks=") {
            expand_all_hunks = val == "true";
        } else if let Some(val) = line.strip_prefix("classic_layout=") {
            classic_layout = val == "true";
        } else if let Some(val) = line.strip_prefix("local_repo_root=") {
            if !val.trim().is_empty() {
                local_repo_roots.push(val.trim().to_string());
            }
        }
    }

    Settings {
        model,
        github_token,
        aws_profile,
        anthropic_api_key,
        provider,
        openai_api_key,
        gemini_api_key,
        typesafe_api_key,
        openai_base_url,
        filter_older,
        filter_team,
        view_mode,
        show_hunk_significance,
        show_ai_notes,
        hunk_filter,
        activity_per_watch_cap,
        activity_mini_player,
        show_activity,
        show_approved_prs,
        show_draft_prs,
        setup_done,
        expand_all_hunks,
        classic_layout,
        local_repo_roots,
    }
}

pub fn save_settings_to_disk(settings: &Settings) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }

    let content = serialize_settings(settings);

    // Atomic + created 0600 from the first byte: the token never touches
    // disk world-readable, even transiently.
    crate::state_io::write_atomic(&path, content.as_bytes())
        .map_err(|e| format!("Failed to save settings: {}", e))?;

    Ok(())
}

/// Render settings in the `key=value` config format (inverse of
/// `parse_settings`). Pure, like its counterpart.
pub fn serialize_settings(settings: &Settings) -> String {
    let mut content = format!("model={}\n", settings.model);
    if !settings.github_token.is_empty() {
        content.push_str(&format!("github_token={}\n", settings.github_token));
    }
    if !settings.aws_profile.is_empty() {
        content.push_str(&format!("aws_profile={}\n", settings.aws_profile));
    }
    if !settings.anthropic_api_key.is_empty() {
        content.push_str(&format!("anthropic_api_key={}\n", settings.anthropic_api_key));
    }
    if !settings.provider.is_empty() {
        content.push_str(&format!("provider={}\n", settings.provider));
    }
    if !settings.openai_api_key.is_empty() {
        content.push_str(&format!("openai_api_key={}\n", settings.openai_api_key));
    }
    if !settings.gemini_api_key.is_empty() {
        content.push_str(&format!("gemini_api_key={}\n", settings.gemini_api_key));
    }
    if !settings.typesafe_api_key.is_empty() {
        content.push_str(&format!("typesafe_api_key={}\n", settings.typesafe_api_key));
    }
    if !settings.openai_base_url.is_empty() {
        content.push_str(&format!("openai_base_url={}\n", settings.openai_base_url));
    }
    content.push_str(&format!("filter_older={}\n", settings.filter_older));
    content.push_str(&format!("filter_team={}\n", settings.filter_team));
    content.push_str(&format!("view_mode={}\n", settings.view_mode));
    content.push_str(&format!("show_hunk_significance={}\n", settings.show_hunk_significance));
    content.push_str(&format!("show_ai_notes={}\n", settings.show_ai_notes));
    content.push_str(&format!("hunk_filter={}\n", settings.hunk_filter));
    content.push_str(&format!(
        "activity_per_watch_cap={}\n",
        settings.activity_per_watch_cap
    ));
    content.push_str(&format!(
        "activity_mini_player={}\n",
        settings.activity_mini_player
    ));
    content.push_str(&format!("show_activity={}\n", settings.show_activity));
    content.push_str(&format!("show_approved_prs={}\n", settings.show_approved_prs));
    content.push_str(&format!("show_draft_prs={}\n", settings.show_draft_prs));
    content.push_str(&format!("setup_done={}\n", settings.setup_done));
    content.push_str(&format!("expand_all_hunks={}\n", settings.expand_all_hunks));
    content.push_str(&format!("classic_layout={}\n", settings.classic_layout));
    for root in settings.local_repo_roots.iter().map(|r| r.trim()).filter(|r| !r.is_empty() && !r.contains('\n')) {
        content.push_str(&format!("local_repo_root={}\n", root));
    }
    content
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

/// Resolve the Anthropic API key: config file > ANTHROPIC_API_KEY env. Returns
/// None if unset (then the backend falls back to the `claude` CLI).
pub fn resolve_anthropic_api_key(settings: &Settings) -> Option<String> {
    resolve_secret(&settings.anthropic_api_key, "ANTHROPIC_API_KEY")
}

/// Resolve the OpenAI (or OpenAI-compatible) API key: config > OPENAI_API_KEY.
pub fn resolve_openai_api_key(settings: &Settings) -> Option<String> {
    resolve_secret(&settings.openai_api_key, "OPENAI_API_KEY")
}

/// Resolve the TypeSafe API key for Jev: config > TYPESAFE_API_KEY.
pub fn resolve_jev_api_key(settings: &Settings) -> Option<String> {
    resolve_secret(&settings.typesafe_api_key, "TYPESAFE_API_KEY")
}

/// Resolve the Gemini API key: config > GEMINI_API_KEY.
pub fn resolve_gemini_api_key(settings: &Settings) -> Option<String> {
    resolve_secret(&settings.gemini_api_key, "GEMINI_API_KEY")
}

/// Resolve the OpenAI-compatible base URL: config > OPENAI_BASE_URL. None = use
/// the provider's built-in default.
pub fn resolve_openai_base_url(settings: &Settings) -> Option<String> {
    resolve_secret(&settings.openai_base_url, "OPENAI_BASE_URL")
}

/// A config-field-or-env value: the field wins; else the env var if non-empty.
fn resolve_secret(field: &str, env_var: &str) -> Option<String> {
    if !field.is_empty() {
        return Some(field.to_string());
    }
    match env::var(env_var) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
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
    fn anthropic_key_resolves_from_config_field() {
        // A config-file key short-circuits before any env lookup, so this is
        // deterministic regardless of the test environment's ANTHROPIC_API_KEY.
        let mut s = default_settings();
        s.anthropic_api_key = "sk-ant-from-config".to_string();
        assert_eq!(resolve_anthropic_api_key(&s).as_deref(), Some("sk-ant-from-config"));
    }

    #[test]
    fn jev_key_round_trips_and_is_only_written_when_set() {
        let mut s = default_settings();
        assert!(!serialize_settings(&s).contains("typesafe_api_key"));
        s.typesafe_api_key = "ts-from-config".to_string();
        let text = serialize_settings(&s);
        assert!(text.contains("typesafe_api_key=ts-from-config\n"));
        let back = parse_settings(&text);
        assert_eq!(resolve_jev_api_key(&back).as_deref(), Some("ts-from-config"));
    }

    #[test]
    fn a_jev_key_saved_under_the_draft_name_carries_over() {
        let s = parse_settings("model=\nvercel_ai_gateway_api_key=ts-old\n");
        assert_eq!(s.typesafe_api_key, "ts-old");
        let text = serialize_settings(&s);
        assert!(text.contains("typesafe_api_key=ts-old\n") && !text.contains("vercel_ai_gateway"));
        // The current name wins if both are present.
        assert_eq!(parse_settings("typesafe_api_key=new\nvercel_ai_gateway_api_key=old\n").typesafe_api_key, "new");
    }

    #[test]
    fn activity_is_hidden_by_default_and_gates_the_floating_window() {
        // Fresh install and a config from before the setting existed.
        assert!(!default_settings().show_activity);
        assert!(!parse_settings("model=\nactivity_mini_player=true\n").show_activity);
        let mut s = default_settings();
        // The floating window's own toggle alone doesn't bring it back.
        assert!(s.activity_mini_player && !floating_mini_player_enabled(&s));
        s.show_activity = true;
        assert!(floating_mini_player_enabled(&s));
        let text = serialize_settings(&s);
        assert!(text.contains("show_activity=true\n"));
        assert!(parse_settings(&text).show_activity);
        s.activity_mini_player = false;
        assert!(!floating_mini_player_enabled(&s));
    }

    #[test]
    fn inbox_is_the_default_layout() {
        // Fresh install, a config written before the setting existed, and one
        // from a preview build that saved `inbox_layout=false` on every save.
        assert!(!default_settings().classic_layout);
        assert!(!parse_settings("model=\nview_mode=split\n").classic_layout);
        assert!(!parse_settings("model=\ninbox_layout=false\n").classic_layout);
    }

    #[test]
    fn classic_layout_round_trips_through_the_config_format() {
        let mut s = default_settings();
        s.classic_layout = true;
        let text = serialize_settings(&s);
        assert!(text.contains("classic_layout=true\n"));
        assert!(parse_settings(&text).classic_layout);
        s.classic_layout = false;
        assert!(!parse_settings(&serialize_settings(&s)).classic_layout);
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
