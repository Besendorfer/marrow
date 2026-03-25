use crate::bedrock::{extract_json_array, region_from_arn, BedrockClient};
use crate::config::resolve_github_token;
use crate::github::GithubClient;
use crate::pr_parser::parse_pr_ref;
use crate::prompts::{build_classification_prompt, build_grouping_prompt, build_highlight_prompt, build_summary_prompt};
use crate::types::{
    ChangeGroup, FetchProgress, FetchStatus, FileClassification, FileDiff, Highlight, HighlightResult, ReviewManifest, Settings,
};
use futures::stream::{FuturesUnordered, StreamExt};
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use tauri::Emitter;

fn emit_progress(
    app: &tauri::AppHandle,
    step: u8,
    label: &str,
    status: FetchStatus,
    pr_title: Option<&str>,
    files: Option<(u32, u32)>,
) {
    let _ = app.emit("fetch-progress", FetchProgress {
        step,
        total_steps: 6,
        label: label.to_string(),
        status,
        pr_title: pr_title.map(|s| s.to_string()),
        files_done: files.map(|(d, _)| d),
        files_total: files.map(|(_, t)| t),
    });
}

pub async fn fetch_pr_impl(pr_ref: &str, settings: &Settings, app: &tauri::AppHandle) -> Result<ReviewManifest, String> {
    if settings.model.is_empty() {
        return Err("No model ARN configured. Set it in Settings.".to_string());
    }

    let token = resolve_github_token(settings)?;
    let parsed = parse_pr_ref(pr_ref)?;

    let github = GithubClient::new(token);

    // Step 1: Fetch PR metadata
    emit_progress(app, 1, "Fetching PR metadata", FetchStatus::Running, None, None);
    let metadata = github
        .get_pr_metadata(&parsed.owner, &parsed.repo, parsed.number)
        .await?;
    let pr_title = metadata.title;
    emit_progress(app, 1, "Fetching PR metadata", FetchStatus::Done, Some(&pr_title), None);
    let pr_url = metadata.html_url;
    let pr_number = metadata.number;
    let base_ref = metadata.base.ref_name;
    let head_ref = metadata.head.ref_name;
    let base_sha = metadata.base.sha;
    let head_sha = metadata.head.sha;

    // Step 2: Fetch PR file list and diff in parallel
    emit_progress(app, 2, "Fetching files and diff", FetchStatus::Running, None, None);
    let (files_result, diff_result) = tokio::join!(
        github.get_pr_files(&parsed.owner, &parsed.repo, parsed.number),
        github.get_pr_diff(&parsed.owner, &parsed.repo, parsed.number),
    );

    let pr_files = files_result?;
    let full_diff = diff_result?;
    emit_progress(app, 2, "Fetching files and diff", FetchStatus::Done, None, None);

    if pr_files.is_empty() {
        return Err("No changed files found in this PR.".to_string());
    }

    let file_list: Vec<String> = pr_files.iter().map(|f| f.filename.clone()).collect();

    // Index additions/deletions by filename for later use
    let file_stats: HashMap<String, (u64, u64)> = pr_files
        .iter()
        .map(|f| (f.filename.clone(), (f.additions, f.deletions)))
        .collect();

    // Step 3: AI classification
    emit_progress(app, 3, "Classifying files with AI", FetchStatus::Running, None, None);
    let region = region_from_arn(&settings.model)?;
    let bedrock = BedrockClient::new(&region, &settings.aws_profile).await?;

    let classification_prompt =
        build_classification_prompt(&pr_title, &file_list, &full_diff);

    let classification_raw = bedrock
        .invoke_model(&settings.model, &classification_prompt)
        .await?;

    let classification_json = extract_json_array(&classification_raw)?;
    let classifications: Vec<FileClassification> = serde_json::from_value(classification_json)
        .map_err(|e| format!("Failed to parse classification: {}", e))?;
    emit_progress(app, 3, "Classifying files with AI", FetchStatus::Done, None, None);

    let relevant: Vec<&FileClassification> = classifications
        .iter()
        .filter(|c| c.classification == "RELEVANT")
        .collect();

    if relevant.is_empty() {
        return Ok(ReviewManifest {
            pr_title,
            pr_url,
            pr_number,
            base_ref,
            head_ref,
            base_sha,
            head_sha,
            summary: String::new(),
            change_groups: vec![],
            files: vec![],
        });
    }

    // Step 4: AI highlight analysis + summary + grouping (parallel)
    emit_progress(app, 4, "Analyzing highlights, summary, and grouping", FetchStatus::Running, None, Some((0, 3)));
    let per_file_diff_map = build_per_file_diff_map(&full_diff);
    let per_file_diffs = extract_per_file_diffs(&per_file_diff_map, &relevant);
    let highlight_prompt = build_highlight_prompt(&pr_title, &per_file_diffs);

    let summary_prompt = build_summary_prompt(&pr_title, &relevant);
    let grouping_prompt = build_grouping_prompt(&pr_title, &relevant);

    let mut ai_stream: FuturesUnordered<_> = [
        ("highlights", bedrock.invoke_model(&settings.model, &highlight_prompt)),
        ("summary", bedrock.invoke_model(&settings.model, &summary_prompt)),
        ("grouping", bedrock.invoke_model(&settings.model, &grouping_prompt)),
    ].into_iter().map(|(name, fut)| async move { (name, fut.await) }).collect();

    let mut highlights_raw = Err("not started".to_string());
    let mut summary_raw = Err("not started".to_string());
    let mut grouping_raw = Err("not started".to_string());
    let mut ai_done: u32 = 0;
    while let Some((name, result)) = ai_stream.next().await {
        ai_done += 1;
        emit_progress(app, 4, "Analyzing highlights, summary, and grouping", FetchStatus::Running, None, Some((ai_done, 3)));
        match name {
            "highlights" => highlights_raw = result,
            "summary" => summary_raw = result,
            "grouping" => grouping_raw = result,
            _ => {}
        }
    }
    emit_progress(app, 4, "Analyzing highlights, summary, and grouping", FetchStatus::Done, None, None);

    let highlights_raw = highlights_raw.unwrap_or_else(|_| "[]".to_string());

    let highlights_json = extract_json_array(&highlights_raw).unwrap_or_else(|_| {
        serde_json::Value::Array(vec![])
    });

    let highlight_results: Vec<HighlightResult> =
        serde_json::from_value(highlights_json).unwrap_or_default();

    let summary = summary_raw.unwrap_or_default();

    let change_groups: Vec<ChangeGroup> = grouping_raw
        .ok()
        .and_then(|raw| extract_json_array(&raw).ok())
        .and_then(|json| serde_json::from_value(json).ok())
        .unwrap_or_default();

    // Index highlights by file path
    let mut highlights_by_path: HashMap<String, Vec<Highlight>> = HashMap::new();
    for h in highlight_results {
        highlights_by_path
            .entry(h.path.clone())
            .or_default()
            .push(Highlight {
                start_line: h.start_line,
                end_line: h.end_line,
                severity: h.severity,
                comment: h.comment,
            });
    }

    // Step 5: Fetch file contents for all relevant files concurrently
    let files_total = relevant.len() as u32;
    emit_progress(app, 5, "Fetching file contents", FetchStatus::Running, None, Some((0, files_total)));
    let content_futures: Vec<_> = relevant
        .iter()
        .map(|f| {
            let path = f.path.clone();
            let owner = parsed.owner.clone();
            let repo = parsed.repo.clone();
            let base = base_sha.clone();
            let head = head_sha.clone();
            let gh = &github;
            async move {
                let (base_content, head_content) = tokio::join!(
                    gh.get_file_content(&owner, &repo, &path, &base),
                    gh.get_file_content(&owner, &repo, &path, &head),
                );
                (path, base_content, head_content)
            }
        })
        .collect();

    let mut stream: FuturesUnordered<_> = content_futures.into_iter().collect();
    let mut contents = Vec::with_capacity(files_total as usize);
    let mut files_done: u32 = 0;
    while let Some(result) = stream.next().await {
        files_done += 1;
        emit_progress(app, 5, "Fetching file contents", FetchStatus::Running, None, Some((files_done, files_total)));
        contents.push(result);
    }
    emit_progress(app, 5, "Fetching file contents", FetchStatus::Done, None, None);

    // Step 6: Build the manifest
    emit_progress(app, 6, "Building review manifest", FetchStatus::Running, None, None);
    let mut file_diffs = Vec::new();

    for (path, base_result, head_result) in &contents {
        let base_content = base_result.as_deref().unwrap_or("").to_string();
        let head_content = head_result.as_deref().unwrap_or("").to_string();

        let diff_type = if base_content.is_empty() && !head_content.is_empty() {
            "added"
        } else if !base_content.is_empty() && head_content.is_empty() {
            "removed"
        } else {
            "modified"
        };

        // Get the unified diff for this file, stripping the git header
        // For added/removed files, split large single hunks into smaller chunks
        let unified_diff = per_file_diff_map
            .get(path)
            .map(|d| {
                let stripped = strip_diff_header(d);
                if diff_type == "added" || diff_type == "removed" {
                    split_single_hunk(&stripped)
                } else {
                    stripped
                }
            })
            .unwrap_or_default();

        let classification = classifications
            .iter()
            .find(|c| c.path == *path)
            .unwrap();

        let (additions, deletions) = file_stats.get(path).copied().unwrap_or((0, 0));

        // Build hunk_scores using heuristic analysis of each hunk's content
        let hunk_lines = split_diff_into_hunk_lines(&unified_diff);
        let hunk_scores: Vec<String> = hunk_lines
            .iter()
            .map(|lines| heuristic_significance(lines, path))
            .collect();

        let diff_hash = {
            let mut hasher = Sha256::new();
            hasher.update(unified_diff.as_bytes());
            format!("{:x}", hasher.finalize())[..16].to_string()
        };

        file_diffs.push(FileDiff {
            path: path.clone(),
            classification: classification.classification.clone(),
            reason: classification.reason.clone(),
            category: classification.category.clone(),
            risk_level: classification.risk_level.clone(),
            diff_type: diff_type.to_string(),
            base_content,
            head_content,
            unified_diff,
            additions,
            deletions,
            highlights: highlights_by_path.remove(path.as_str()).unwrap_or_default(),
            hunk_scores,
            diff_hash,
        });
    }

    emit_progress(app, 6, "Building review manifest", FetchStatus::Done, None, None);

    Ok(ReviewManifest {
        pr_title,
        pr_url,
        pr_number,
        base_ref,
        head_ref,
        base_sha,
        head_sha,
        summary,
        change_groups,
        files: file_diffs,
    })
}

/// Extract per-file diffs for the relevant files from a pre-built diff map.
fn extract_per_file_diffs(
    diff_map: &HashMap<String, String>,
    relevant: &[&FileClassification],
) -> Vec<(String, String)> {
    let mut result = Vec::new();

    for f in relevant {
        if let Some(diff) = diff_map.get(&f.path) {
            result.push((f.path.clone(), diff.clone()));
        }
    }

    result
}

/// Parse the full unified diff into a map of file_path -> diff_text.
fn build_per_file_diff_map(full_diff: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut current_path: Option<String> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in full_diff.lines() {
        if line.starts_with("diff --git ") {
            // Save previous file
            if let Some(path) = current_path.take() {
                map.insert(path, current_lines.join("\n"));
            }
            current_lines.clear();

            // Extract path from "diff --git a/path b/path"
            if let Some(b_part) = line.split(" b/").last() {
                current_path = Some(b_part.to_string());
            }
        }
        current_lines.push(line);
    }

    // Save the last file
    if let Some(path) = current_path {
        map.insert(path, current_lines.join("\n"));
    }

    map
}

/// For added/removed files with a single large hunk, split into multiple
/// synthetic hunks at blank-line boundaries (targeting ~30 lines per chunk).
/// This lets the AI score each section independently.
fn split_single_hunk(diff: &str) -> String {
    let lines: Vec<&str> = diff.lines().collect();

    // Count @@ lines — only split if there's exactly one hunk
    let hunk_count = lines.iter().filter(|l| l.starts_with("@@")).count();
    if hunk_count != 1 {
        return diff.to_string();
    }

    // Find the content lines (after the @@ header)
    let hunk_start = lines.iter().position(|l| l.starts_with("@@"));
    let hunk_start = match hunk_start {
        Some(i) => i,
        None => return diff.to_string(),
    };

    // Determine the prefix (+ or -)
    let prefix = lines[hunk_start + 1..].iter().find(|l| l.starts_with('+') || l.starts_with('-'));
    let prefix_char = match prefix {
        Some(l) if l.starts_with('+') => '+',
        Some(l) if l.starts_with('-') => '-',
        _ => return diff.to_string(),
    };

    let content_lines = &lines[hunk_start + 1..];
    if content_lines.len() < 40 {
        // Too short to bother splitting
        return diff.to_string();
    }

    // Find split points: blank lines (just the prefix with nothing after)
    let mut split_points: Vec<usize> = Vec::new();
    let target_chunk = 30;
    let mut since_last_split = 0;

    for (i, line) in content_lines.iter().enumerate() {
        since_last_split += 1;
        let is_blank = (line.len() == 1 && line.starts_with(prefix_char)) || line.trim().is_empty();
        if is_blank && since_last_split >= target_chunk {
            split_points.push(i);
            since_last_split = 0;
        }
    }

    if split_points.is_empty() {
        return diff.to_string();
    }

    // Rebuild the diff with synthetic @@ headers at split points
    let mut result = Vec::new();

    // Emit first @@ header with correct chunk size
    let first_chunk_size = split_points[0] + 1;
    if prefix_char == '+' {
        result.push(format!("@@ -0,0 +1,{} @@", first_chunk_size));
    } else {
        result.push(format!("@@ -1,{} +0,0 @@", first_chunk_size));
    }

    let mut current_line_num: u64 = 1;
    let mut chunk_start = 0;

    for (sp_idx, &split_at) in split_points.iter().enumerate() {
        // Emit lines from chunk_start to split_at (inclusive)
        for line in &content_lines[chunk_start..=split_at] {
            result.push(line.to_string());
        }
        current_line_num += (split_at - chunk_start + 1) as u64;
        chunk_start = split_at + 1;

        // Calculate the size of the next chunk
        let next_split = if sp_idx + 1 < split_points.len() {
            split_points[sp_idx + 1] + 1
        } else {
            content_lines.len()
        };
        let next_chunk_size = next_split - chunk_start;

        // Emit synthetic @@ header
        if next_chunk_size > 0 {
            if prefix_char == '+' {
                result.push(format!("@@ -0,0 +{},{} @@", current_line_num, next_chunk_size));
            } else {
                result.push(format!("@@ -{},{} +0,0 @@", current_line_num, next_chunk_size));
            }
        }
    }

    // Emit remaining lines
    for line in &content_lines[chunk_start..] {
        result.push(line.to_string());
    }

    result.join("\n")
}

/// Split a unified diff into per-hunk content lines (excluding @@ headers).
fn split_diff_into_hunk_lines(diff: &str) -> Vec<Vec<String>> {
    let mut hunks: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();

    for line in diff.lines() {
        if line.starts_with("@@") {
            if !current.is_empty() || !hunks.is_empty() {
                hunks.push(current);
            }
            current = Vec::new();
        } else {
            current.push(line.to_string());
        }
    }
    if !current.is_empty() {
        hunks.push(current);
    }
    hunks
}

/// Detect the language family from a file path extension.
fn lang_from_path(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => "js",
        "py" => "python",
        "rs" => "rust",
        "go" => "go",
        "java" | "kt" | "scala" => "jvm",
        "rb" => "ruby",
        "cs" => "csharp",
        "cpp" | "c" | "h" | "hpp" => "cpp",
        "sh" | "bash" | "zsh" => "shell",
        _ => "js", // default to JS patterns since that's the primary codebase
    }
}

/// Check if a line is a structural bracket/paren line (possibly with keywords).
/// Catches patterns like `}: Props)`, `}) => {`, `export default function Foo() {`, etc.
fn is_structural_line(trimmed: &str) -> bool {
    // Pure bracket lines are already caught as exact matches in the trivial check.
    // This catches lines that are *mostly* structural with a bit of decoration.
    let stripped: String = trimmed
        .chars()
        .filter(|c| !matches!(c, '{' | '}' | '(' | ')' | '[' | ']' | ';' | ',' | ' ' | ':'))
        .collect();
    // After removing brackets/punctuation/spaces, if very little remains, it's structural
    stripped.len() <= 3
}

/// Check if a line is JSX/template markup noise.
/// `lower` is the pre-computed lowercase version of `trimmed`.
fn is_jsx_noise(trimmed: &str, lower: &str) -> bool {
    // Lines that are purely JSX tags or props
    if trimmed.starts_with('<') && !trimmed.contains('{') {
        return true; // plain HTML/JSX tag like `<div>`, `</div>`, `<Component />`
    }
    if trimmed.starts_with('<') && trimmed.ends_with('>') {
        return true;
    }
    // JSX prop lines: `className="..."`, `onClick={handler}`, `style={{...}}`
    if lower.starts_with("classname")
        || lower.starts_with("style=")
        || lower.starts_with("aria-")
        || lower.starts_with("data-")
        || lower.starts_with("role=")
        || lower.starts_with("key=")
        || lower.starts_with("ref=")
        || lower.starts_with("id=")
    {
        return true;
    }
    // Closing JSX fragments
    if trimmed == "</>" || trimmed == "<>" || trimmed.starts_with("</") {
        return true;
    }
    false
}

/// Language-specific trivial line detection.
fn is_lang_trivial(trimmed: &str, lower: &str, lang: &str) -> bool {
    match lang {
        "rust" => {
            trimmed.starts_with("use ")
                || trimmed.starts_with("mod ")
                || trimmed.starts_with("pub mod ")
                || trimmed.starts_with("pub use ")
                || trimmed.starts_with("pub(crate)")
                || trimmed.starts_with("#[")
                || trimmed.starts_with("///")
                || trimmed.starts_with("//!")
        }
        "python" => {
            trimmed.starts_with("from ")
                || trimmed.starts_with("import ")
                || trimmed.starts_with("# ")
                || trimmed == "#"
                || trimmed.starts_with("\"\"\"")
                || trimmed.starts_with("'''")
                || lower.starts_with("pass")
                || lower.starts_with("@")
        }
        "go" => {
            trimmed.starts_with("import ")
                || trimmed.starts_with("import (")
                || trimmed == "import ("
                || trimmed.starts_with("// ")
                || trimmed.starts_with("package ")
        }
        "jvm" => {
            trimmed.starts_with("import ")
                || trimmed.starts_with("package ")
                || trimmed.starts_with("@")
                || trimmed.starts_with("//")
                || trimmed.starts_with("/*")
                || trimmed.starts_with("* ")
                || trimmed.starts_with("*/")
        }
        "ruby" => {
            trimmed.starts_with("require ")
                || trimmed.starts_with("require_relative ")
                || trimmed.starts_with("include ")
                || trimmed.starts_with("# ")
                || trimmed == "end"
        }
        "csharp" => {
            trimmed.starts_with("using ")
                || trimmed.starts_with("namespace ")
                || trimmed.starts_with("//")
                || trimmed.starts_with("[")  // attributes like [HttpGet]
        }
        "cpp" => {
            trimmed.starts_with("#include")
                || trimmed.starts_with("#pragma")
                || trimmed.starts_with("#define")
                || trimmed.starts_with("#ifndef")
                || trimmed.starts_with("#endif")
                || trimmed.starts_with("//")
                || trimmed.starts_with("/*")
                || trimmed.starts_with("using namespace")
        }
        "shell" => {
            trimmed.starts_with("# ")
                || trimmed == "#"
                || trimmed.starts_with("#!/")
                || trimmed.starts_with("set ")
                || trimmed.starts_with("export ")
        }
        _ => false, // JS/TS trivial patterns are already in the main check
    }
}

/// Check if `haystack` contains `word` as a standalone word (not as a substring
/// of a larger identifier). A word boundary is any non-alphanumeric, non-underscore
/// character, or the start/end of the string. This prevents "token_count" from
/// matching "token", or "assign(" from matching "sign(".
fn contains_word(haystack: &str, word: &str) -> bool {
    let bytes = haystack.as_bytes();
    let word_bytes = word.as_bytes();
    let wlen = word_bytes.len();
    if bytes.len() < wlen {
        return false;
    }
    for i in 0..=(bytes.len() - wlen) {
        if &bytes[i..i + wlen] == word_bytes {
            let before_ok = i == 0 || !is_ident_char(bytes[i - 1]);
            let after_ok = i + wlen == bytes.len() || !is_ident_char(bytes[i + wlen]);
            if before_ok && after_ok {
                return true;
            }
        }
    }
    false
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Heuristic scoring for a hunk based on content analysis.
///
/// Improvements over a naive keyword check:
/// 1. Two keyword tiers: critical (auth, DB, security) vs normal logic (if, return)
/// 2. Structural/bracket-only lines detected as trivial
/// 3. JSX/template noise detected as trivial
/// 4. Language-aware trivial detection (Rust, Python, Go, etc.)
/// 5. Hunk size as a factor: tiny hunks capped at medium, large hunks bumped up
/// 6. Diff direction: removed critical keywords score double (deleted safety checks)
/// 7. Weighted scoring: ratio of signal lines to non-trivial lines
fn heuristic_significance(lines: &[String], path: &str) -> String {
    let lang = lang_from_path(path);
    let mut total_changes: u64 = 0;
    let mut trivial_changes: u64 = 0;
    let mut critical_score: f64 = 0.0;
    let mut logic_hits: u64 = 0;

    for line in lines {
        // Only look at actual change lines
        let (prefix, rest) = if line.starts_with('+') {
            ('+', &line[1..])
        } else if line.starts_with('-') {
            ('-', &line[1..])
        } else {
            continue;
        };

        total_changes += 1;
        let trimmed = rest.trim();

        // ── Trivial detection ───────────────────────────────────────────────

        // Universal trivial: blank lines, comments, single brackets
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("*/")
            || trimmed.starts_with("console.")
            || trimmed.starts_with("logger.")
            || trimmed == "}"
            || trimmed == "})"
            || trimmed == "});"
            || trimmed == ");"
            || trimmed == "{"
            || trimmed == "("
            || trimmed == "),"
            || trimmed == "]"
            || trimmed == "],"
            || trimmed == "["
            || trimmed == "else {"
            || trimmed == "} else {"
        {
            trivial_changes += 1;
            continue;
        }

        // JS/TS-specific trivial (always checked since it's the primary codebase)
        if trimmed.starts_with("import ")
            || trimmed.starts_with("import{")
            || trimmed.starts_with("from ")
            || trimmed.starts_with("require(")
            || trimmed.starts_with("export type ")
            || trimmed.starts_with("export interface ")
            || trimmed.starts_with("interface ")
            || trimmed.starts_with("type ")
            || trimmed.starts_with("} from ")
            || trimmed.starts_with("export {")
            || trimmed.starts_with("export default")
            || trimmed.starts_with("export *")
            || trimmed.starts_with("@")
            || trimmed == "} as const;"
            || trimmed == "} as const"
            || trimmed.ends_with("as const;")
        {
            trivial_changes += 1;
            continue;
        }

        let lower = trimmed.to_lowercase();

        // Language-specific trivial
        if is_lang_trivial(trimmed, &lower, lang) {
            trivial_changes += 1;
            continue;
        }

        // Structural bracket lines (e.g. `}: Props)`, `}) => {`)
        if is_structural_line(trimmed) {
            trivial_changes += 1;
            continue;
        }

        // JSX/template noise
        if is_jsx_noise(trimmed, &lower) {
            trivial_changes += 1;
            continue;
        }

        // ── Signal detection ────────────────────────────────────────────────

        // Removed lines with critical keywords are extra dangerous (deleted safety checks)
        let direction_weight: f64 = if prefix == '-' { 1.5 } else { 1.0 };

        // Critical keywords: security, DB mutations, auth, error handling.
        // Use word-boundary-aware matching to avoid false positives like
        // "token_count" matching "token" or "assign(" matching "sign(".
        if lower.contains("authenticate")
            || lower.contains("authorize")
            || lower.contains("password")
            || contains_word(&lower, "secret")
            || lower.contains("permission")
            || contains_word(&lower, "token")
            || lower.contains(".delete(")
            || lower.contains(".remove(")
            || lower.contains(".destroy(")
            || lower.contains(".drop(")
            || lower.contains(".update(")
            || lower.contains(".insert(")
            || lower.contains(".save(")
            || lower.contains(".exec()")
            || lower.contains(".aggregate(")
            || lower.contains("throw ")
            || lower.contains("new error")
            || contains_word(&lower, "migration")
            || contains_word(&lower, "middleware")
            || contains_word(&lower, "cors")
            || lower.contains("csrf")
            || lower.contains("encrypt")
            || lower.contains("decrypt")
            || lower.contains(".hash(")
            || lower.contains(".sign(")
            || lower.contains(".verify(")
        {
            critical_score += direction_weight;
        }
        // Normal logic keywords: common control flow, not inherently risky
        else if lower.contains("if (")
            || lower.contains("if(")
            || lower.contains("return ")
            || lower.contains("await ")
            || lower.contains("async ")
            || lower.contains("switch ")
            || lower.contains("catch ")
            || lower.contains("try {")
            || lower.contains(".then(")
            || lower.contains("promise.")
            || lower.contains(".find(")
            || lower.contains(".findone(")
            || lower.contains(".filter(")
            || lower.contains(".map(")
        {
            logic_hits += 1;
        }
    }

    if total_changes == 0 {
        return "low".to_string();
    }

    let trivial_ratio = trivial_changes as f64 / total_changes as f64;

    // If 80%+ of changes are trivial, it's low regardless
    if trivial_ratio >= 0.8 {
        return "low".to_string();
    }

    let non_trivial = total_changes - trivial_changes;
    let change_lines = total_changes; // for size-based adjustments

    // ── Size-based adjustments ──────────────────────────────────────────

    // Tiny hunks (≤5 change lines): cap at medium even with critical keywords.
    // A single `return token` in a 3-line hunk is trivially reviewable.
    if change_lines <= 5 {
        if critical_score > 0.0 {
            return "medium".to_string();
        }
        // Tiny hunk with only normal logic or plain code → low
        return "low".to_string();
    }

    // ── Score determination ─────────────────────────────────────────────

    // Critical keywords present → high (weighted by direction)
    if critical_score >= 1.0 {
        return "high".to_string();
    }

    // Weighted ratio: what fraction of non-trivial lines are logic keywords?
    let logic_ratio = logic_hits as f64 / non_trivial as f64;

    // Large hunks (50+ change lines) with moderate logic get bumped to medium
    if change_lines >= 50 && logic_ratio >= 0.15 {
        return "medium".to_string();
    }

    if logic_ratio >= 0.3 {
        "medium".to_string()
    } else {
        "low".to_string()
    }
}

/// Strip the git diff header (everything before the first @@ line).
fn strip_diff_header(diff: &str) -> String {
    let mut lines = diff.lines();
    let mut result = Vec::new();
    let mut found_hunk = false;

    for line in &mut lines {
        if line.starts_with("@@") {
            found_hunk = true;
        }
        if found_hunk {
            result.push(line);
        }
    }

    result.join("\n")
}
