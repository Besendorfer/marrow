//! `marrow` — a thin terminal frontend over the `marrow-core` Rust core.
//!
//! This is a prototype to evaluate a TUI/CLI direction. It deliberately reuses
//! the exact same modules the desktop app's Tauri commands call — `github.rs`,
//! `fetch.rs`, `config.rs` — so it proves how much of the app is already
//! frontend-agnostic. No webview, no Tauri runtime: just the core + stdout.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{IsTerminal, Write};
use std::sync::OnceLock;

use clap::{Parser, Subcommand, ValueEnum};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::as_24_bit_terminal_escaped;

use marrow_core::config::{
    app_config_dir, config_path, load_settings, resolve_anthropic_api_key, resolve_gemini_api_key,
    resolve_github_token, resolve_openai_api_key, resolve_openai_base_url,
};
use marrow_core::fetch::fetch_pr_impl;
use marrow_core::github::GithubClient;
use marrow_core::types::{FetchProgress, FetchStatus, FileDiff, Highlight, ReviewManifest, ReviewThread};

mod tui;

/// When to colorize output. `auto` = colorize only when stdout is a terminal.
#[derive(Copy, Clone, Debug, ValueEnum)]
enum ColorWhen {
    Auto,
    Always,
    Never,
}

#[derive(Parser)]
#[command(
    name = "marrow",
    about = "Marrow in your terminal — surface the PR changes that matter",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Skip the confirmation prompt for mutating commands (reply, resolve,
    /// approve, …). Required when stdin is not a terminal.
    #[arg(short = 'y', long, global = true)]
    yes: bool,

    /// When to colorize output: auto (TTY only), always, or never.
    #[arg(long, value_enum, default_value_t = ColorWhen::Auto, global = true)]
    color: ColorWhen,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch + classify a PR and print the review manifest
    Review {
        /// PR reference: URL, owner/repo/pull/N, or owner/repo#N
        pr: String,
        /// Emit the full manifest as JSON instead of formatted text
        #[arg(long)]
        json: bool,
        /// Also print the unified diff for each file
        #[arg(long)]
        diffs: bool,
        /// Force the printed report instead of the interactive TUI
        #[arg(long)]
        no_tui: bool,
    },
    /// Print the PR's raw unified diff (relevance-ordered, no chrome) — pipe into nvim/delta
    Diff {
        /// PR reference: URL, owner/repo/pull/N, or owner/repo#N
        pr: String,
    },
    /// List PRs awaiting your review
    Requests {
        /// How many days back to search
        #[arg(long, default_value_t = 60)]
        days: i64,
        #[arg(long)]
        json: bool,
    },
    /// Show review comment threads on a PR
    Comments {
        pr: String,
        /// Only show unresolved threads
        #[arg(long)]
        unresolved: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show CI check status for a PR
    Checks {
        pr: String,
        #[arg(long)]
        json: bool,
    },
    /// Reply to a review comment thread
    Reply {
        pr: String,
        /// Comment node ID to reply to (shown by `marrow comments`)
        comment_id: String,
        /// Reply body
        body: String,
    },
    /// Mark a review thread resolved
    Resolve {
        /// Thread node ID (shown by `marrow comments`)
        thread_id: String,
    },
    /// Reopen a resolved review thread
    Unresolve {
        thread_id: String,
    },
    /// Approve a PR
    Approve {
        pr: String,
        /// Optional approval message
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Request changes on a PR
    #[command(name = "request-changes")]
    RequestChanges {
        pr: String,
        /// Review body explaining the requested changes
        #[arg(short, long)]
        message: String,
    },
    /// Submit a comment-only review (no approval state)
    #[command(name = "comment-review")]
    CommentReview {
        pr: String,
        #[arg(short, long)]
        message: String,
    },
    /// Start a new inline review comment on a file line
    Comment {
        pr: String,
        /// File path as it appears in the diff
        path: String,
        /// Line number to attach the comment to
        line: u64,
        /// Comment body
        body: String,
        /// Which side of the diff: RIGHT (new/head) or LEFT (old/base)
        #[arg(long, default_value = "RIGHT")]
        side: String,
        /// For a multi-line comment, the first line of the range
        #[arg(long)]
        start_line: Option<u64>,
        /// Side for the start line (defaults to --side)
        #[arg(long)]
        start_side: Option<String>,
    },
    /// Add a top-level conversation comment to a PR (not a review, not anchored to a line)
    #[command(name = "pr-comment")]
    PrComment {
        pr: String,
        #[arg(short, long)]
        message: String,
    },
    /// Print resolved settings (token source is masked)
    Settings,
    /// Create the config directory and a starter config file (migrates the
    /// pre-rename dir if present)
    Init,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let use_color = match cli.color {
        ColorWhen::Always => true,
        ColorWhen::Never => false,
        ColorWhen::Auto => std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
    };
    let _ = COLOR.set(use_color);
    if let Err(e) = run(cli.command, cli.yes).await {
        eprintln!("{}", paint(&format!("error: {e}"), RED));
        std::process::exit(1);
    }
}

fn github_client() -> GithubClient {
    let settings = load_settings();
    GithubClient::new(resolve_github_token(&settings))
}

async fn run(command: Command, yes: bool) -> Result<(), String> {
    match command {
        Command::Review { pr, json, diffs, no_tui } => review(&pr, json, diffs, no_tui).await,
        Command::Diff { pr } => diff_cmd(&pr).await,
        Command::Requests { days, json } => requests(days, json).await,
        Command::Comments { pr, unresolved, json } => comments(&pr, unresolved, json).await,
        Command::Checks { pr, json } => checks(&pr, json).await,
        Command::Reply { pr, comment_id, body } => {
            confirm(&format!("Reply to a thread on {pr}?"), yes)?;
            reply(&pr, &comment_id, &body).await
        }
        Command::Resolve { thread_id } => {
            confirm(&format!("Resolve thread {thread_id}?"), yes)?;
            set_resolved(&thread_id, true).await
        }
        Command::Unresolve { thread_id } => {
            confirm(&format!("Reopen thread {thread_id}?"), yes)?;
            set_resolved(&thread_id, false).await
        }
        Command::Approve { pr, message } => {
            confirm(&format!("Approve {pr}?"), yes)?;
            submit(&pr, "APPROVE", message.as_deref().unwrap_or("")).await
        }
        Command::RequestChanges { pr, message } => {
            confirm(&format!("Request changes on {pr}?"), yes)?;
            submit(&pr, "REQUEST_CHANGES", &message).await
        }
        Command::CommentReview { pr, message } => {
            confirm(&format!("Submit a comment review on {pr}?"), yes)?;
            submit(&pr, "COMMENT", &message).await
        }
        Command::Comment { pr, path, line, body, side, start_line, start_side } => {
            confirm(&format!("Comment on {pr} {path}:{line}?"), yes)?;
            comment(&pr, &path, line, &body, &side, start_line, start_side.as_deref()).await
        }
        Command::PrComment { pr, message } => {
            confirm(&format!("Comment on the {pr} conversation?"), yes)?;
            pr_comment(&pr, &message).await
        }
        Command::Settings => {
            settings_cmd();
            Ok(())
        }
        Command::Init => init_cmd(),
    }
}

/// Gate a mutating action behind confirmation. With `--yes`, proceeds silently.
/// Interactively, prompts on stderr. When stdin is not a terminal and `--yes`
/// was not given, refuses rather than mutating a real PR unattended.
fn confirm(action: &str, yes: bool) -> Result<(), String> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "{action} refusing to run without --yes (stdin is not a terminal)"
        ));
    }
    eprint!("{} {} ", paint(action, BOLD), paint("[y/N]", DIM));
    let _ = std::io::stderr().flush();
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(|e| e.to_string())?;
    match input.trim().to_lowercase().as_str() {
        "y" | "yes" => Ok(()),
        _ => Err("aborted".to_string()),
    }
}

// ── mutations ────────────────────────────────────────────────────────────────

async fn reply(pr: &str, comment_id: &str, body: &str) -> Result<(), String> {
    let github = github_client();
    let parsed = marrow_core::pr_parser::parse_pr_ref(pr)?;
    let pr_node_id = github
        .get_pull_request_id(&parsed.owner, &parsed.repo, parsed.number)
        .await?;
    let c = github.reply_to_review_thread(&pr_node_id, comment_id, body).await?;
    println!("{} replied as @{}  {}", paint("✓", GREEN), c.author.login, paint(&c.url, DIM));
    Ok(())
}

async fn set_resolved(thread_id: &str, resolve: bool) -> Result<(), String> {
    let github = github_client();
    let now_resolved = github.resolve_review_thread(thread_id, resolve).await?;
    let label = if now_resolved { paint("resolved", GREEN) } else { paint("reopened", YELLOW) };
    println!("{} thread {label}", paint("✓", GREEN));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn comment(
    pr: &str,
    path: &str,
    line: u64,
    body: &str,
    side: &str,
    start_line: Option<u64>,
    start_side: Option<&str>,
) -> Result<(), String> {
    let github = github_client();
    let parsed = marrow_core::pr_parser::parse_pr_ref(pr)?;
    let pr_node_id = github
        .get_pull_request_id(&parsed.owner, &parsed.repo, parsed.number)
        .await?;
    let side = side.to_uppercase();
    // A range comment needs a start side too; default it to the same side.
    let eff_start_side: Option<String> = start_line
        .map(|_| start_side.map(|s| s.to_uppercase()).unwrap_or_else(|| side.clone()));
    let thread = github
        .create_review_thread(&pr_node_id, body, path, line, &side, start_line, eff_start_side.as_deref())
        .await?;
    let url = thread.comments.first().map(|c| c.url.as_str()).unwrap_or("");
    println!("{} comment posted on {path}:{line}  {}", paint("✓", GREEN), paint(url, DIM));
    Ok(())
}

async fn pr_comment(pr: &str, body: &str) -> Result<(), String> {
    let github = github_client();
    let parsed = marrow_core::pr_parser::parse_pr_ref(pr)?;
    let url = github
        .add_pr_comment(&parsed.owner, &parsed.repo, parsed.number, body)
        .await?;
    println!("{} comment posted  {}", paint("✓", GREEN), paint(&url, DIM));
    Ok(())
}

async fn submit(pr: &str, event: &str, body: &str) -> Result<(), String> {
    let github = github_client();
    let parsed = marrow_core::pr_parser::parse_pr_ref(pr)?;
    let state = github
        .submit_review(&parsed.owner, &parsed.repo, parsed.number, event, body)
        .await?;
    let colored = match state.as_str() {
        "APPROVED" => paint(&state, GREEN),
        "CHANGES_REQUESTED" => paint(&state, RED),
        _ => paint(&state, YELLOW),
    };
    println!("{} review submitted: {colored}", paint("✓", GREEN));
    Ok(())
}

// ── review ──────────────────────────────────────────────────────────────────

async fn review(pr: &str, json: bool, show_diffs: bool, no_tui: bool) -> Result<(), String> {
    let settings = load_settings();
    let manifest = fetch_pr_impl(pr, &settings, &fetch_progress_to_stderr).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?);
        return Ok(());
    }

    // Interactive default: open the TUI on a terminal unless raw diffs were
    // requested or it was opted out. Piped/non-TTY falls back to the report.
    if !show_diffs && !no_tui && std::io::stdout().is_terminal() {
        return tui::run(&manifest).map_err(|e| e.to_string());
    }

    let mut out = String::new();
    print_manifest(&mut out, &manifest, show_diffs);
    page_or_print(&out);
    Ok(())
}

/// Forward fetch progress to stderr so stdout stays clean for piping/--json.
fn fetch_progress_to_stderr(p: FetchProgress) {
    if matches!(p.status, FetchStatus::Running) {
        let mut line = format!("[{}/{}] {}", p.step, p.total_steps, p.label);
        if let (Some(d), Some(t)) = (p.files_done, p.files_total) {
            line.push_str(&format!(" ({d}/{t})"));
        }
        eprintln!("{}", paint(&line, DIM));
    }
}

// ── diff (raw unified diff for piping into nvim/delta) ───────────────────────

async fn diff_cmd(pr: &str) -> Result<(), String> {
    let settings = load_settings();
    let manifest = fetch_pr_impl(pr, &settings, &fetch_progress_to_stderr).await?;

    // Order by risk so the files that matter come first — the differentiator
    // over a plain `gh pr diff`.
    let mut files: Vec<&FileDiff> = manifest
        .files
        .iter()
        .filter(|f| !f.unified_diff.is_empty())
        .collect();
    files.sort_by_key(|f| risk_order(&f.risk_level));

    let mut out = String::new();
    for f in files {
        write_file_patch(&mut out, f);
    }
    print!("{out}");
    Ok(())
}

fn risk_order(risk: &str) -> u8 {
    match risk {
        "high" => 0,
        "low" => 2,
        _ => 1,
    }
}

/// Emit a standard unified-diff section for one file. The core stores
/// header-stripped hunks, so synthesize the `diff --git` + `---`/`+++` headers,
/// using /dev/null for added/removed files so the patch stays valid.
fn write_file_patch(out: &mut String, f: &FileDiff) {
    let _ = writeln!(out, "diff --git a/{} b/{}", f.path, f.path);
    match f.diff_type.as_str() {
        "added" => {
            let _ = writeln!(out, "--- /dev/null");
            let _ = writeln!(out, "+++ b/{}", f.path);
        }
        "removed" => {
            let _ = writeln!(out, "--- a/{}", f.path);
            let _ = writeln!(out, "+++ /dev/null");
        }
        _ => {
            let _ = writeln!(out, "--- a/{}", f.path);
            let _ = writeln!(out, "+++ b/{}", f.path);
        }
    }
    out.push_str(&f.unified_diff);
    if !f.unified_diff.ends_with('\n') {
        out.push('\n');
    }
}

/// Print to stdout, paging through $PAGER (default `less -R`) when stdout is a
/// terminal. Falls back to a plain print when not a TTY or no pager is found.
fn page_or_print(content: &str) {
    if std::io::stdout().is_terminal() && try_page(content).is_ok() {
        return;
    }
    print!("{content}");
}

fn try_page(content: &str) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less -R".to_string());
    let mut parts = pager.split_whitespace();
    let cmd = parts
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "empty PAGER"))?;
    let mut child = Command::new(cmd).args(parts).stdin(Stdio::piped()).spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(content.as_bytes());
        // stdin dropped here → EOF to the pager
    }
    child.wait()?;
    Ok(())
}

fn print_manifest<W: std::fmt::Write>(out: &mut W, m: &ReviewManifest, show_diffs: bool) {
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", paint(&format!("{} #{}", m.pr_title, m.pr_number), BOLD));
    let _ = writeln!(out, "{}", paint(&m.pr_url, DIM));
    let _ = writeln!(out, "{}", paint(&format!("{} ← {}", m.base_ref, m.head_ref), DIM));

    if !m.summary.is_empty() {
        let _ = writeln!(out);
        for line in wrap(&m.summary, 88) {
            let _ = writeln!(out, "  {line}");
        }
    }

    if !m.change_groups.is_empty() {
        let _ = writeln!(out, "\n{}", paint("Change groups", BOLD));
        for g in &m.change_groups {
            let _ = writeln!(out, "  {} {}", paint("●", CYAN), paint(&g.label, BOLD));
            for line in wrap(&g.description, 84) {
                let _ = writeln!(out, "      {}", paint(&line, DIM));
            }
            let _ = writeln!(out, "      {}", paint(&format!("{} file(s)", g.file_paths.len()), DIM));
        }
    }

    let _ = writeln!(out, "\n{} {}", paint("Files", BOLD), paint(&format!("({})", m.files.len()), DIM));
    for f in &m.files {
        let risk = match f.risk_level.as_str() {
            "high" => paint("HIGH", RED),
            "low" => paint("low ", GREEN),
            _ => paint("med ", YELLOW),
        };
        let churn = paint(&format!("+{} -{}", f.additions, f.deletions), DIM);
        let _ = writeln!(out, "  {risk} {}  {churn}", f.path);
        let meta = format!("{} · {}", f.classification, f.category);
        let _ = writeln!(out, "        {}", paint(&meta, DIM));
        if !f.reason.is_empty() {
            for line in wrap(&f.reason, 80) {
                let _ = writeln!(out, "        {}", paint(&line, DIM));
            }
        }
        // When showing diffs, highlights are rendered inline in the diff, so
        // skip the summary list to avoid duplication.
        if !show_diffs {
            for h in &f.highlights {
                let sev = severity_color(&h.severity);
                let loc = if h.start_line == h.end_line {
                    format!("L{}", h.start_line)
                } else {
                    format!("L{}-{}", h.start_line, h.end_line)
                };
                let _ = writeln!(out, "        {} {} {}", paint("▸", sev), paint(&loc, sev), h.comment);
            }
        }
        if show_diffs && !f.unified_diff.is_empty() {
            let _ = writeln!(out);
            render_file_diff(out, f);
            let _ = writeln!(out);
        }
    }
    let _ = writeln!(out);
}

// ── diff rendering (syntect highlighting + AI-highlight annotations) ─────────

fn severity_color(sev: &str) -> &'static str {
    match sev {
        "high" | "warning" => RED,
        "medium" => YELLOW,
        _ => CYAN,
    }
}

fn sev_rank(c: &str) -> u8 {
    match c {
        RED => 3,
        YELLOW => 2,
        _ => 1,
    }
}

fn highlighter() -> &'static (SyntaxSet, Theme) {
    static HL: OnceLock<(SyntaxSet, Theme)> = OnceLock::new();
    HL.get_or_init(|| {
        let ps = SyntaxSet::load_defaults_newlines();
        let ts = ThemeSet::load_defaults();
        let theme = ts.themes["base16-ocean.dark"].clone();
        (ps, theme)
    })
}

fn syntax_for<'a>(ps: &'a SyntaxSet, path: &str) -> &'a SyntaxReference {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    ps.find_syntax_by_extension(ext)
        .unwrap_or_else(|| ps.find_syntax_plain_text())
}

/// Syntax-highlight one line of code to 24-bit ANSI (fg only). Each line is
/// highlighted independently — good enough for a diff view, where lines arrive
/// out of file order (a perfect render is Level 2 / a `delta`-style rebuild).
fn highlight_line(code: &str, syntax: &SyntaxReference) -> String {
    let (ps, theme) = highlighter();
    let mut h = HighlightLines::new(syntax, theme);
    match h.highlight_line(code, ps) {
        Ok(ranges) => {
            let mut out = as_24_bit_terminal_escaped(&ranges, false);
            out.push_str("\x1b[0m");
            out
        }
        Err(_) => code.to_string(),
    }
}

/// Parse the new-side start line from a hunk header: `@@ -a,b +c,d @@` -> c.
fn hunk_new_start(header: &str) -> Option<u64> {
    header
        .split_whitespace()
        .find(|t| t.starts_with('+'))
        .and_then(|t| t.trim_start_matches('+').split(',').next())
        .and_then(|n| n.parse::<u64>().ok())
}

/// Render a file's unified diff with syntax highlighting and inline AI-highlight
/// annotations: a severity gutter bar on flagged lines plus the comment printed
/// above its start line. Falls back to plain text when color is disabled.
fn render_file_diff<W: std::fmt::Write>(out: &mut W, f: &FileDiff) {
    let indent = "  ";
    let color = color_enabled();

    // Map new-side line numbers to the highlights that start there, and the set
    // of new-side lines covered by any highlight (for the gutter bar).
    let mut starts: HashMap<u64, Vec<&Highlight>> = HashMap::new();
    let mut covered: HashMap<u64, &'static str> = HashMap::new();
    for h in &f.highlights {
        starts.entry(h.start_line).or_default().push(h);
        let sev = severity_color(&h.severity);
        for ln in h.start_line..=h.end_line {
            covered
                .entry(ln)
                .and_modify(|c| {
                    if sev_rank(sev) > sev_rank(c) {
                        *c = sev;
                    }
                })
                .or_insert(sev);
        }
    }

    let (ps, _) = highlighter();
    let syntax = syntax_for(ps, &f.path);

    let mut new_ln: Option<u64> = None;
    for line in f.unified_diff.lines() {
        if line.starts_with("@@") {
            new_ln = hunk_new_start(line);
            let _ = writeln!(out, "{indent}{}", paint(line, CYAN));
            continue;
        }

        let (marker, rest, on_new_side, marker_color) = match line.chars().next() {
            Some('+') => ('+', &line[1..], true, GREEN),
            Some('-') => ('-', &line[1..], false, RED),
            Some(' ') => (' ', &line[1..], true, DIM),
            _ => {
                // "\ No newline at end of file", stray blank lines, etc.
                let _ = writeln!(out, "{indent} {}", paint(line, DIM));
                continue;
            }
        };

        let cur_new = if on_new_side { new_ln } else { None };

        // Annotation: print the comment(s) just above the line they start on.
        if let Some(ln) = cur_new {
            if let Some(hs) = starts.get(&ln) {
                for h in hs {
                    let sev = severity_color(&h.severity);
                    let loc = if h.start_line == h.end_line {
                        format!("L{}", h.start_line)
                    } else {
                        format!("L{}-{}", h.start_line, h.end_line)
                    };
                    let _ = writeln!(out, "{indent}{} {} {}", paint("▸", sev), paint(&loc, sev), h.comment);
                }
            }
        }

        let gutter = match cur_new.and_then(|ln| covered.get(&ln)) {
            Some(sev) => paint("▍", sev),
            None => " ".to_string(),
        };
        let marker_str = if color {
            paint(&marker.to_string(), marker_color)
        } else {
            marker.to_string()
        };
        let code = if color { highlight_line(rest, syntax) } else { rest.to_string() };
        let _ = writeln!(out, "{indent}{gutter}{marker_str}{code}");

        if on_new_side {
            if let Some(n) = new_ln.as_mut() {
                *n += 1;
            }
        }
    }
}

// ── requests ────────────────────────────────────────────────────────────────

async fn requests(days: i64, json: bool) -> Result<(), String> {
    let github = github_client();
    let username = github.get_authenticated_user().await?;
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string();
    let items = github.get_review_requests(&username, &cutoff, true).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?);
        return Ok(());
    }

    if items.is_empty() {
        println!("No review requests in the last {days} days.");
        return Ok(());
    }

    println!("{}", paint(&format!("Review requests for {username} ({} found)", items.len()), BOLD));
    for it in &items {
        let flag = if it.direct_request { paint("@you", CYAN) } else { paint("team", DIM) };
        let status = match it.my_review_status.as_str() {
            "APPROVED" => paint("approved", GREEN),
            "CHANGES_REQUESTED" => paint("changes", RED),
            "" | "PENDING" => paint("pending", YELLOW),
            other => paint(other, DIM),
        };
        println!(
            "  {} {}  {}",
            flag,
            status,
            paint(&format!("{}/{} #{}", it.owner, it.repo, it.number), BOLD)
        );
        println!("      {}", it.title);
        let mut tail = format!("by {}", it.author);
        if it.unresolved_thread_count > 0 {
            tail.push_str(&format!(" · {} unresolved", it.unresolved_thread_count));
        }
        println!("      {}", paint(&tail, DIM));
    }
    Ok(())
}

// ── comments ────────────────────────────────────────────────────────────────

async fn comments(pr: &str, unresolved_only: bool, json: bool) -> Result<(), String> {
    let github = github_client();
    let parsed = marrow_core::pr_parser::parse_pr_ref(pr)?;
    let mut threads = github
        .get_review_threads(&parsed.owner, &parsed.repo, parsed.number)
        .await?;

    if unresolved_only {
        threads.retain(|t| !t.is_resolved);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&threads).map_err(|e| e.to_string())?);
        return Ok(());
    }

    print_threads(&threads);
    Ok(())
}

fn print_threads(threads: &[ReviewThread]) {
    if threads.is_empty() {
        println!("No threads.");
        return;
    }
    for t in threads {
        let state = if t.is_resolved {
            paint("✓ resolved", GREEN)
        } else {
            paint("○ open", YELLOW)
        };
        let loc = t.line.map(|l| format!(":{l}")).unwrap_or_default();
        let outdated = if t.is_outdated { paint(" (outdated)", DIM) } else { String::new() };
        println!("\n{state} {}{}{}", paint(&t.path, BOLD), loc, outdated);
        println!("  {}", paint(&format!("thread {}", t.id), DIM));
        for c in &t.comments {
            println!(
                "  {}  {}",
                paint(&format!("@{}", c.author.login), CYAN),
                paint(&format!("reply-to {}", c.id), DIM)
            );
            for line in wrap(&c.body, 84) {
                println!("    {line}");
            }
        }
    }
    println!();
}

// ── checks ──────────────────────────────────────────────────────────────────

async fn checks(pr: &str, json: bool) -> Result<(), String> {
    let github = github_client();
    let parsed = marrow_core::pr_parser::parse_pr_ref(pr)?;
    let status = github
        .get_pr_checks(&parsed.owner, &parsed.repo, parsed.number)
        .await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&status).map_err(|e| e.to_string())?);
        return Ok(());
    }

    let overall = match status.overall_state.as_str() {
        "SUCCESS" => paint(&status.overall_state, GREEN),
        "FAILURE" | "ERROR" => paint(&status.overall_state, RED),
        _ => paint(&status.overall_state, YELLOW),
    };
    println!("Overall: {overall}");
    for c in &status.check_runs {
        let mark = match c.conclusion.as_deref() {
            Some("SUCCESS") => paint("✓", GREEN),
            Some("FAILURE") | Some("TIMED_OUT") | Some("CANCELLED") => paint("✗", RED),
            Some("SKIPPED") | Some("NEUTRAL") => paint("•", DIM),
            _ => paint("◌", YELLOW),
        };
        let detail = c.conclusion.clone().unwrap_or_else(|| c.status.clone());
        println!("  {mark} {}  {}", c.name, paint(&detail, DIM));
    }
    Ok(())
}

// ── settings ────────────────────────────────────────────────────────────────

/// A commented starter config written by `marrow init` when none exists.
const CONFIG_TEMPLATE: &str = "\
# Marrow config — https://github.com/Besendorfer/marrow
# (No inline comments on value lines — everything after `=` is the value.)
# GitHub personal access token (optional for public repos; or GH_TOKEN / GITHUB_TOKEN).
github_token=
# AI model. The provider is auto-detected from the name: claude* -> Anthropic,
# gpt*/o3* -> OpenAI, gemini* -> Gemini; or an AWS Bedrock model ARN.
model=
# API key for your model (or the matching env var). Set the one you need:
#   anthropic_api_key  for claude*  (or ANTHROPIC_API_KEY)
#   openai_api_key     for gpt*/o*  (or OPENAI_API_KEY)
#   gemini_api_key     for gemini*  (or GEMINI_API_KEY)
anthropic_api_key=
openai_api_key=
gemini_api_key=
# Optional: override auto-detect. provider=openai-compatible (e.g. OpenRouter or
# a local server) then set openai_base_url + openai_api_key.
provider=
openai_base_url=
# AWS profile name (only used with a Bedrock ARN model).
aws_profile=
";

/// Ensure the config directory exists (migrating the pre-rename dir if present)
/// and scaffold a starter config file. Idempotent — never clobbers an existing
/// config.
fn init_cmd() -> Result<(), String> {
    // Resolving the dir also runs the one-time legacy migration.
    let dir = app_config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("create config dir: {e}"))?;

    let cfg = config_path();
    let created = !cfg.exists();
    if created {
        std::fs::write(&cfg, CONFIG_TEMPLATE).map_err(|e| format!("write config: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o600));
        }
    }

    println!("{} config dir: {}", paint("✓", GREEN), dir.display());
    let note = if created { "wrote starter config" } else { "config already exists" };
    println!("  {note}: {}", paint(&cfg.display().to_string(), DIM));
    println!();
    println!(
        "Add your GitHub token to the config, or set {} / {}.",
        paint("GH_TOKEN", CYAN),
        paint("GITHUB_TOKEN", CYAN)
    );
    println!("Then run {} to verify.", paint("marrow settings", CYAN));
    Ok(())
}

fn settings_cmd() {
    let s = load_settings();
    let token = resolve_github_token(&s);
    let token_status = match token {
        Some(t) if !t.is_empty() => paint(&format!("set ({}…)", &t.chars().take(4).collect::<String>()), GREEN),
        _ => paint("not set", RED),
    };
    let key_status = |k: Option<String>| match k {
        Some(v) if !v.is_empty() => paint("set", GREEN),
        _ => paint("not set", DIM),
    };
    // Which backend a `marrow review` would use with this config.
    let backend = marrow_core::ai::provider_for_settings(&s);
    let base_url = resolve_openai_base_url(&s);
    println!("model:          {}", if s.model.is_empty() { paint("(none)", RED) } else { s.model.clone() });
    println!("github_token:   {}", token_status);
    println!("anthropic_key:  {}", key_status(resolve_anthropic_api_key(&s)));
    println!("openai_key:     {}", key_status(resolve_openai_api_key(&s)));
    println!("gemini_key:     {}", key_status(resolve_gemini_api_key(&s)));
    if let Some(url) = &base_url {
        println!("openai_base:    {url}");
    }
    if !s.provider.is_empty() {
        println!("provider:       {} (override)", s.provider);
    }
    println!("aws_profile:    {}", if s.aws_profile.is_empty() { paint("(none)", DIM) } else { s.aws_profile.clone() });
    println!(
        "ai backend:     {}",
        if s.model.is_empty() { paint("(model not set)", RED) } else { backend.label().to_string() }
    );
    println!("view_mode:      {}", s.view_mode);
    println!("hunk_filter:    {}", s.hunk_filter);
}

// ── tiny ANSI + wrapping helpers (no extra deps) ─────────────────────────────

const BOLD: &str = "1";
const DIM: &str = "2";
const RED: &str = "31";
const GREEN: &str = "32";
const YELLOW: &str = "33";
const CYAN: &str = "36";

static COLOR: OnceLock<bool> = OnceLock::new();

fn color_enabled() -> bool {
    *COLOR.get().unwrap_or(&false)
}

fn paint(s: &str, code: &str) -> String {
    if color_enabled() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// Naive word wrap at `width` columns. Good enough for a prototype; a real TUI
/// would use a layout engine.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if !current.is_empty() && current.len() + 1 + word.len() > width {
                lines.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        lines.push(current);
    }
    // Drop a trailing empty line artifact but keep intentional blank paragraphs.
    let _ = std::io::stdout().flush();
    lines
}
