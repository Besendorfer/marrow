//! Read-only access to a local clone of the PR's repo (issue #232).
//!
//! GitHub code search only indexes the default branch (so it can't see code
//! a PR adds) and is rate-limited to ~10 requests/min. When the reviewer has
//! the repo cloned under one of `Settings.local_repo_roots` and the clone
//! already contains the PR's commits, the review's repo tools read and search
//! it with `git show` / `git grep` / `git ls-tree` at exact SHAs instead.
//!
//! Strictly read-only: this module never fetches, checks out, or writes to
//! the clone. Every git invocation reads objects at an explicit commit, so
//! the working tree and index are never consulted or touched. If a commit
//! isn't present locally, the caller falls back to GitHub.

use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// Per-git-call deadline — a wedged git must not stall the review.
const GIT_TIMEOUT: Duration = Duration::from_secs(20);
/// How deep under each root to look for clones (root/org/repo covers the
/// common `~/code/<org>/<repo>` layout).
const DISCOVERY_DEPTH: usize = 2;
/// Max grep hits reported back to the model.
pub const GREP_MAX_HITS: usize = 30;

/// A local clone that can serve the PR's commits.
#[derive(Debug, Clone)]
pub struct LocalClone {
    pub dir: PathBuf,
}

async fn git(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let fut = Command::new("git")
        .arg("-C")
        .arg(dir)
        // Never prompt, never page, never read the user's global config
        // hooks — this is a background read.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .args(args)
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(GIT_TIMEOUT, fut).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(format!("git failed to start: {e}")),
        Err(_) => Err("git timed out".to_string()),
    }
}

/// Parse `owner/repo` (lowercased) out of a GitHub remote URL, in either
/// https or ssh form. `None` for non-GitHub remotes.
pub fn parse_github_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = rest.split('/');
    let owner = parts.next()?.to_lowercase();
    let repo = parts.next()?.to_lowercase();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((owner, repo))
}

/// Candidate clone directories under `roots` (the roots themselves plus
/// subdirectories up to `DISCOVERY_DEPTH`) that contain a `.git` entry.
fn candidate_dirs(roots: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut frontier: Vec<(PathBuf, usize)> = roots
        .iter()
        .filter(|r| !r.trim().is_empty())
        .map(|r| (expand_home(r.trim()), 0))
        .collect();
    while let Some((dir, depth)) = frontier.pop() {
        if dir.join(".git").exists() {
            out.push(dir);
            continue; // don't descend into a repo
        }
        if depth >= DISCOVERY_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let hidden = p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with('.')).unwrap_or(true);
            if !hidden && p.is_dir() {
                frontier.push((p, depth + 1));
            }
        }
    }
    out
}

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(rest),
        None => PathBuf::from(p),
    }
}

/// Find a clone of `owner/repo` under `roots` whose object store already
/// contains every commit in `required_shas`. Remote matching is on `origin`
/// only. `None` means "use GitHub".
pub async fn find_clone(roots: &[String], owner: &str, repo: &str, required_shas: &[&str]) -> Option<LocalClone> {
    let want = (owner.to_lowercase(), repo.to_lowercase());
    for dir in candidate_dirs(roots) {
        let Ok(out) = git(&dir, &["remote", "get-url", "origin"]).await else { continue };
        if !out.status.success() {
            continue;
        }
        let url = String::from_utf8_lossy(&out.stdout);
        if parse_github_remote(&url).as_ref() != Some(&want) {
            continue;
        }
        let mut all = true;
        for sha in required_shas {
            if !has_commit(&dir, sha).await {
                all = false;
                break;
            }
        }
        if all {
            return Some(LocalClone { dir });
        }
    }
    None
}

async fn has_commit(dir: &Path, sha: &str) -> bool {
    if !is_hex_sha(sha) {
        return false;
    }
    let spec = format!("{sha}^{{commit}}");
    matches!(git(dir, &["cat-file", "-e", &spec]).await, Ok(o) if o.status.success())
}

fn is_hex_sha(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

impl LocalClone {
    /// File contents at `sha`. `Ok("")` when the path doesn't exist there
    /// (mirrors `GithubClient::get_file_content`'s 404 convention).
    pub async fn read_file(&self, sha: &str, path: &str) -> Result<String, String> {
        if !is_hex_sha(sha) {
            return Err("invalid commit".to_string());
        }
        let spec = format!("{sha}:{path}");
        let out = git(&self.dir, &["show", &spec]).await?;
        if !out.status.success() {
            return Ok(String::new());
        }
        String::from_utf8(out.stdout).map_err(|_| format!("{path} is not valid UTF-8 (binary file?)"))
    }

    /// Fixed-string, case-insensitive search of every text file at `sha`.
    /// Returns `path:line:text` rows (commit prefix stripped), at most
    /// `GREP_MAX_HITS`, plus whether more were cut.
    pub async fn grep(&self, sha: &str, query: &str) -> Result<(Vec<String>, bool), String> {
        if !is_hex_sha(sha) {
            return Err("invalid commit".to_string());
        }
        // -e keeps a query starting with '-' from parsing as an option.
        let out = git(&self.dir, &["grep", "-n", "-I", "-i", "-F", "-e", query, sha, "--"]).await?;
        // Exit 1 = no matches; anything else non-zero is a real error.
        match out.status.code() {
            Some(0) => {}
            Some(1) => return Ok((Vec::new(), false)),
            _ => return Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        }
        let prefix = format!("{sha}:");
        let text = String::from_utf8_lossy(&out.stdout);
        let all: Vec<String> = text
            .lines()
            .map(|l| l.strip_prefix(&prefix).unwrap_or(l).to_string())
            .collect();
        let more = all.len() > GREP_MAX_HITS;
        Ok((all.into_iter().take(GREP_MAX_HITS).collect(), more))
    }

    /// Directory entries at `sha` as (name, type, size). `path` empty = root.
    pub async fn list_dir(&self, sha: &str, path: &str) -> Result<Vec<(String, String, u64)>, String> {
        if !is_hex_sha(sha) {
            return Err("invalid commit".to_string());
        }
        let treeish = if path.is_empty() { sha.to_string() } else { format!("{sha}:{path}") };
        let out = git(&self.dir, &["ls-tree", "-l", &treeish]).await?;
        if !out.status.success() {
            return Err(format!("not a directory: {path}"));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text
            .lines()
            .filter_map(|l| {
                // "<mode> <type> <sha> <size>\t<name>"
                let (meta, name) = l.split_once('\t')?;
                let mut f = meta.split_whitespace();
                let _mode = f.next()?;
                let kind = f.next()?;
                let _obj = f.next()?;
                let size = f.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let kind = if kind == "tree" { "dir" } else { "file" };
                Some((name.to_string(), kind.to_string(), size))
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = StdCommand::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A throwaway repo under the system temp dir with one commit and a
    /// GitHub-style origin. Returns (root containing it, repo dir, head sha).
    fn temp_repo(tag: &str) -> (PathBuf, PathBuf, String) {
        // Unique per run; temp dirs are left for the OS to reap (no cleanup
        // deletes in tests).
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("marrow-local-repo-{tag}-{}-{nanos}", std::process::id()));
        let dir = root.join("acme").join("api");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        sh(&dir, &["init", "-q"]);
        sh(&dir, &["config", "user.email", "t@example.com"]);
        sh(&dir, &["config", "user.name", "t"]);
        sh(&dir, &["remote", "add", "origin", "git@github.com:Acme/API.git"]);
        std::fs::write(dir.join("src/lib.rs"), "pub fn charge(cents: u64) {}\n// -flag\n").unwrap();
        sh(&dir, &["add", "."]);
        sh(&dir, &["commit", "-q", "-m", "init"]);
        let head = sh(&dir, &["rev-parse", "HEAD"]);
        (root, dir, head)
    }

    #[test]
    fn remote_urls_parse_in_every_github_form() {
        let want = Some(("acme".to_string(), "api".to_string()));
        assert_eq!(parse_github_remote("git@github.com:Acme/API.git"), want);
        assert_eq!(parse_github_remote("https://github.com/acme/api"), want);
        assert_eq!(parse_github_remote("https://github.com/acme/api.git/\n"), want);
        assert_eq!(parse_github_remote("ssh://git@github.com/acme/api.git"), want);
        assert_eq!(parse_github_remote("https://gitlab.com/acme/api"), None);
        assert_eq!(parse_github_remote("https://github.com/acme"), None);
    }

    #[tokio::test]
    async fn finds_clone_and_reads_greps_lists_at_sha_without_writing() {
        let (root, dir, head) = temp_repo("read");
        let roots = vec![root.to_string_lossy().into_owned()];
        let status_before = sh(&dir, &["status", "--porcelain"]);
        let refs_before = sh(&dir, &["for-each-ref"]);

        let clone = find_clone(&roots, "acme", "api", &[&head]).await.expect("clone found");
        assert_eq!(clone.dir, dir);

        assert!(clone.read_file(&head, "src/lib.rs").await.unwrap().contains("fn charge"));
        assert_eq!(clone.read_file(&head, "src/missing.rs").await.unwrap(), "");

        let (hits, more) = clone.grep(&head, "CHARGE(").await.unwrap();
        assert_eq!(hits, vec!["src/lib.rs:1:pub fn charge(cents: u64) {}".to_string()]);
        assert!(!more);
        // A leading-dash query is a pattern, not an option.
        let (hits, _) = clone.grep(&head, "-flag").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert!(clone.grep(&head, "no such text").await.unwrap().0.is_empty());

        let root_entries = clone.list_dir(&head, "").await.unwrap();
        assert_eq!(root_entries, vec![("src".to_string(), "dir".to_string(), 0)]);
        let src = clone.list_dir(&head, "src").await.unwrap();
        assert_eq!(src[0].0, "lib.rs");
        assert_eq!(src[0].1, "file");

        // Strictly read-only: no working-tree or ref changes.
        assert_eq!(sh(&dir, &["status", "--porcelain"]), status_before);
        assert_eq!(sh(&dir, &["for-each-ref"]), refs_before);
    }

    #[tokio::test]
    async fn missing_sha_or_other_repo_falls_back() {
        let (root, _dir, head) = temp_repo("fallback");
        let roots = vec![root.to_string_lossy().into_owned()];
        let absent = "0123456789abcdef0123456789abcdef01234567";
        assert!(find_clone(&roots, "acme", "api", &[&head, absent]).await.is_none());
        assert!(find_clone(&roots, "acme", "web", &[&head]).await.is_none());
        assert!(find_clone(&[], "acme", "api", &[&head]).await.is_none());
        // Non-hex "shas" are rejected before reaching git.
        let clone = find_clone(&roots, "acme", "api", &[&head]).await.unwrap();
        assert!(clone.read_file("HEAD --output=/tmp/x", "a").await.is_err());
    }
}
