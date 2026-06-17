//! Interactive TUI — Level 2, M1 skeleton (read-only review viewer).
//!
//! Read-only ⇒ the whole `ReviewManifest` is preloaded before we enter the
//! loop, so the event loop never needs to await. M1 covers: alt-screen in/out,
//! a relevance-ordered file sidebar, the selected file's diff (basic +/-
//! coloring), j/k scrolling, ]/[ file switching, and quit. Syntax highlighting
//! + AI annotations land in M2.

use std::collections::{HashMap, HashSet};
use std::io;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use syntect::easy::HighlightLines;
use syntect::parsing::SyntaxReference;

use marrow_core::types::{FileDiff, Highlight, ReviewManifest, ReviewThread};

/// Enter the alternate screen, run the viewer, and always restore the terminal.
pub fn run(manifest: &ReviewManifest) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::new(manifest);
    let res = app.run(&mut terminal);
    ratatui::restore();
    res
}

#[derive(PartialEq)]
enum Mode {
    Normal,
    Search,
    Help,
    /// Picking a review verb (approve / request-changes / comment).
    ReviewPick,
    /// Typing the review message before submitting.
    ReviewInput,
    /// Typing an inline comment on the cursor line.
    CommentInput,
    /// Typing a reply to the thread under the cursor.
    ReplyInput,
}

/// A commentable diff line: the file line number and the side it's on.
#[derive(Clone, Copy)]
struct CommentTarget {
    line: u64,
    side: &'static str,
}

/// A sidebar row. Group headers are display-only (not selectable).
enum Row {
    Overview,
    Threads,
    Group(String),
    File(usize), // index into App::files
}

struct App<'a> {
    manifest: &'a ReviewManifest,
    files: Vec<&'a FileDiff>,
    list_state: ListState,
    /// Focused row in the main pane; the view scrolls to keep it visible.
    cursor: u16,
    view_top: u16,
    /// Last-rendered viewport height, for half-page / page jumps.
    viewport_h: u16,
    /// Rendered main pane for the current selection (built lazily, not per
    /// frame), plus the row indices of hunk headers and AI findings for jumps,
    /// and a per-row comment target (None for non-commentable rows).
    diff_cache: Vec<Line<'static>>,
    hunk_rows: Vec<usize>,
    finding_rows: Vec<usize>,
    targets: Vec<Option<CommentTarget>>,
    cache_idx: Option<usize>,
    mode: Mode,
    search_input: String,
    rows: Vec<Row>,
    filter_low: bool,
    /// The pending review verb while typing its message (ReviewInput mode).
    review_event: &'static str,
    review_input: String,
    /// The line (range end) being commented on while in CommentInput mode, and
    /// the range start for a multi-line comment (None = single line).
    pending_comment: Option<CommentTarget>,
    pending_comment_start: Option<CommentTarget>,
    /// Anchor row of an active line selection (`v`); the range is anchor..cursor.
    selection_anchor: Option<u16>,
    /// Review threads (None = not loaded yet), the per-row thread index for the
    /// threads view, the per-row thread index for the inline diff (so `r` can
    /// reply to a thread shown in the diff), and the thread being replied to.
    threads: Option<Vec<ReviewThread>>,
    thread_at_row: Vec<Option<usize>>,
    diff_thread_at_row: Vec<Option<usize>>,
    pending_thread: Option<usize>,
    /// Whether we've tried the one-time startup thread load (for inline comments).
    threads_autoloaded: bool,
    /// Last action result, shown in the header.
    status: Option<String>,
}

impl<'a> App<'a> {
    fn new(manifest: &'a ReviewManifest) -> Self {
        // Relevance order: high risk first (matches `marrow diff`).
        let mut files: Vec<&FileDiff> = manifest.files.iter().collect();
        files.sort_by_key(|f| risk_rank(&f.risk_level));
        let mut app = Self {
            manifest,
            files,
            list_state: ListState::default(),
            cursor: 0,
            view_top: 0,
            viewport_h: 20,
            diff_cache: Vec::new(),
            hunk_rows: Vec::new(),
            finding_rows: Vec::new(),
            targets: Vec::new(),
            cache_idx: None,
            mode: Mode::Normal,
            search_input: String::new(),
            rows: Vec::new(),
            filter_low: false,
            review_event: "",
            review_input: String::new(),
            pending_comment: None,
            pending_comment_start: None,
            selection_anchor: None,
            threads: None,
            thread_at_row: Vec::new(),
            diff_thread_at_row: Vec::new(),
            pending_thread: None,
            threads_autoloaded: false,
            status: None,
        };
        app.rebuild_rows();
        // Land on the first (highest-risk) file rather than the overview header.
        let start = app.first_file_row().or_else(|| app.first_selectable());
        app.list_state.select(start);
        app
    }

    fn current_row(&self) -> Option<&Row> {
        self.list_state.selected().and_then(|i| self.rows.get(i))
    }

    fn is_selectable(row: &Row) -> bool {
        !matches!(row, Row::Group(_))
    }

    fn selected_file_idx(&self) -> Option<usize> {
        match self.current_row() {
            Some(Row::File(i)) => Some(*i),
            _ => None,
        }
    }

    fn selected(&self) -> Option<&'a FileDiff> {
        self.selected_file_idx().map(|i| self.files[i])
    }

    fn first_selectable(&self) -> Option<usize> {
        self.rows.iter().position(Self::is_selectable)
    }

    fn first_file_row(&self) -> Option<usize> {
        self.rows.iter().position(|r| matches!(r, Row::File(_)))
    }

    /// Build the sidebar: an overview header, then files grouped by AI
    /// change-group (risk-ordered within), then an "Other" section for any
    /// ungrouped files. Falls back to a flat list with no groups. `filter_low`
    /// hides low-risk files.
    fn build_rows(&self) -> Vec<Row> {
        let filter_low = self.filter_low;
        let keep = |f: &FileDiff| !filter_low || f.risk_level != "low";
        let mut rows = vec![Row::Overview, Row::Threads];

        if self.manifest.change_groups.is_empty() {
            for (i, f) in self.files.iter().enumerate() {
                if keep(f) {
                    rows.push(Row::File(i));
                }
            }
            return rows;
        }

        let mut grouped: HashSet<usize> = HashSet::new();
        for g in &self.manifest.change_groups {
            let members: Vec<usize> = self
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| g.file_paths.contains(&f.path) && keep(f))
                .map(|(i, _)| i)
                .collect();
            if members.is_empty() {
                continue;
            }
            rows.push(Row::Group(g.label.clone()));
            for i in members {
                rows.push(Row::File(i));
                grouped.insert(i);
            }
        }

        let others: Vec<usize> = self
            .files
            .iter()
            .enumerate()
            .filter(|(i, f)| !grouped.contains(i) && keep(f))
            .map(|(i, _)| i)
            .collect();
        if !others.is_empty() {
            rows.push(Row::Group("Other".to_string()));
            for i in others {
                rows.push(Row::File(i));
            }
        }
        rows
    }

    fn rebuild_rows(&mut self) {
        let keep_file = self.selected_file_idx();
        self.rows = self.build_rows();
        self.cache_idx = None; // force re-render after the layout changed
        let target = keep_file
            .and_then(|fi| self.rows.iter().position(|r| matches!(r, Row::File(i) if *i == fi)))
            .or_else(|| self.first_selectable());
        self.list_state.select(target);
        self.cursor = 0;
    }

    fn toggle_filter(&mut self) {
        self.filter_low = !self.filter_low;
        self.rebuild_rows();
    }

    /// Move the selection to the next/previous selectable row, skipping group
    /// headers. Clamps at the ends (no wrap).
    fn select_step(&mut self, forward: bool) {
        let n = self.rows.len();
        let mut i = self.list_state.selected().unwrap_or(0);
        loop {
            if forward {
                if i + 1 >= n {
                    return;
                }
                i += 1;
            } else {
                if i == 0 {
                    return;
                }
                i -= 1;
            }
            if Self::is_selectable(&self.rows[i]) {
                self.list_state.select(Some(i));
                self.cursor = 0;
                self.selection_anchor = None;
                return;
            }
        }
    }

    /// Overview content: risk counts, the PR summary, and change-group blurbs.
    fn build_overview(&self) -> Rendered {
        let mut lines: Vec<Line> = Vec::new();
        let (mut hi, mut me, mut lo) = (0u32, 0u32, 0u32);
        for f in &self.files {
            match f.risk_level.as_str() {
                "high" => hi += 1,
                "low" => lo += 1,
                _ => me += 1,
            }
        }
        lines.push(Line::from(Span::styled(
            format!("{} files · {hi} high · {me} med · {lo} low", self.files.len()),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));

        if !self.manifest.summary.is_empty() {
            for l in crate::wrap(&self.manifest.summary, 80) {
                lines.push(Line::from(l));
            }
            lines.push(Line::from(""));
        }

        if !self.manifest.change_groups.is_empty() {
            lines.push(Line::from(Span::styled(
                "Change groups",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            for g in &self.manifest.change_groups {
                lines.push(Line::from(Span::styled(
                    format!("● {}", g.label),
                    Style::default().fg(Color::Cyan),
                )));
                for l in crate::wrap(&g.description, 78) {
                    lines.push(Line::from(format!("    {l}")));
                }
            }
        }
        Rendered { lines, ..Default::default() }
    }

    fn diff_line_count(&self) -> u16 {
        self.diff_cache.len().min(u16::MAX as usize) as u16
    }

    /// Rebuild the rendered diff (and its nav indices) when the selection changes.
    fn sync_cache(&mut self) {
        let sel = self.list_state.selected();
        if self.cache_idx == sel {
            return;
        }
        self.cache_idx = sel;
        self.thread_at_row = Vec::new();
        self.diff_thread_at_row = Vec::new();
        // Compute the view kind without holding a borrow across the build.
        let is_overview = matches!(self.current_row(), Some(Row::Overview));
        let is_threads = matches!(self.current_row(), Some(Row::Threads));
        let file_idx = self.selected_file_idx();
        let rendered = if is_overview {
            self.build_overview()
        } else if is_threads {
            let (lines, thread_at_row) = self.build_threads_view();
            self.thread_at_row = thread_at_row;
            Rendered { lines, ..Default::default() }
        } else if let Some(i) = file_idx {
            let file = self.files[i];
            if file.unified_diff.is_empty() {
                Rendered::message("(no diff)")
            } else {
                // Open review threads on this file, shown inline in the diff.
                // Carry each thread's global index so the inline rows can map
                // back to `self.threads` for replies.
                let file_threads: Vec<(usize, &ReviewThread)> = self
                    .threads
                    .as_ref()
                    .map(|ts| {
                        ts.iter()
                            .enumerate()
                            .filter(|(_, t)| t.path == file.path && !t.is_resolved)
                            .collect()
                    })
                    .unwrap_or_default();
                diff_lines_for(file, &file_threads)
            }
        } else {
            Rendered::default()
        };
        self.diff_cache = rendered.lines;
        self.hunk_rows = rendered.hunk_rows;
        self.finding_rows = rendered.finding_rows;
        self.targets = rendered.targets;
        self.diff_thread_at_row = rendered.thread_at_row;
    }

    fn jump_next(rows: &[usize], from: u16) -> Option<u16> {
        rows.iter().copied().find(|&r| r as u16 > from).map(|r| r as u16)
    }

    fn jump_prev(rows: &[usize], from: u16) -> Option<u16> {
        rows.iter().copied().rev().find(|&r| (r as u16) < from).map(|r| r as u16)
    }

    fn next_hunk(&mut self) {
        if let Some(r) = Self::jump_next(&self.hunk_rows, self.cursor) {
            self.cursor = r;
        }
    }
    fn prev_hunk(&mut self) {
        if let Some(r) = Self::jump_prev(&self.hunk_rows, self.cursor) {
            self.cursor = r;
        }
    }
    fn next_finding(&mut self) {
        if let Some(r) = Self::jump_next(&self.finding_rows, self.cursor) {
            self.cursor = r;
        }
    }
    fn prev_finding(&mut self) {
        if let Some(r) = Self::jump_prev(&self.finding_rows, self.cursor) {
            self.cursor = r;
        }
    }

    /// Case-insensitive search forward from `start`, wrapping. Returns the row.
    fn find_match(&self, query: &str, start: usize) -> Option<usize> {
        let q = query.to_lowercase();
        let n = self.diff_cache.len();
        if n == 0 || q.is_empty() {
            return None;
        }
        (0..n).find_map(|off| {
            let i = (start + off) % n;
            let text: String =
                self.diff_cache[i].spans.iter().map(|s| s.content.as_ref()).collect();
            text.to_lowercase().contains(&q).then_some(i)
        })
    }

    fn run_search(&mut self) {
        let from = (self.cursor as usize + 1).min(self.diff_cache.len().saturating_sub(1));
        if let Some(i) = self.find_match(&self.search_input, from) {
            self.cursor = i as u16;
        }
    }

    fn cursor_down(&mut self, n: u16) {
        let max = self.diff_line_count().saturating_sub(1);
        self.cursor = (self.cursor + n).min(max);
    }

    fn cursor_up(&mut self, n: u16) {
        self.cursor = self.cursor.saturating_sub(n);
    }

    fn half_page(&self) -> u16 {
        (self.viewport_h / 2).max(1)
    }

    fn full_page(&self) -> u16 {
        self.viewport_h.max(1)
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        loop {
            terminal.draw(|f| self.ui(f))?;
            // After the first frame, load review threads once so they appear
            // inline in the diff without the user pressing T. Redraw afterwards.
            if !self.threads_autoloaded {
                self.threads_autoloaded = true;
                self.ensure_threads_loaded();
                self.cache_idx = None;
                continue;
            }
            // Blocking read — also delivers resize events, which just redraw.
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if self.on_key(key.code, key.modifiers) {
                break;
            }
        }
        Ok(())
    }

    /// Handle one key press. Returns true when the app should quit.
    fn on_key(&mut self, code: KeyCode, mods: KeyModifiers) -> bool {
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        match self.mode {
            Mode::Normal => match code {
                KeyCode::Char('q') => return true,
                KeyCode::Esc => {
                    // Esc cancels an active selection; otherwise it quits.
                    if self.selection_anchor.take().is_none() {
                        return true;
                    }
                }
                KeyCode::Char('v') => {
                    if matches!(self.current_row(), Some(Row::File(_))) {
                        self.selection_anchor =
                            if self.selection_anchor.is_some() { None } else { Some(self.cursor) };
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => self.cursor_down(1),
                KeyCode::Char('k') | KeyCode::Up => self.cursor_up(1),
                // Bulk navigation.
                KeyCode::Char('d') if ctrl => self.cursor_down(self.half_page()),
                KeyCode::Char('u') if ctrl => self.cursor_up(self.half_page()),
                KeyCode::Char('f') if ctrl => self.cursor_down(self.full_page()),
                KeyCode::Char('b') if ctrl => self.cursor_up(self.full_page()),
                KeyCode::PageDown => self.cursor_down(self.full_page()),
                KeyCode::PageUp => self.cursor_up(self.full_page()),
                KeyCode::Home => self.cursor = 0,
                KeyCode::End => self.cursor = self.diff_line_count().saturating_sub(1),
                KeyCode::Char('g') => self.cursor = 0,
                KeyCode::Char('G') => self.cursor = self.diff_line_count().saturating_sub(1),
                KeyCode::Char(']') | KeyCode::Tab => self.select_step(true),
                KeyCode::Char('[') | KeyCode::BackTab => self.select_step(false),
                KeyCode::Char('}') => self.next_hunk(),
                KeyCode::Char('{') => self.prev_hunk(),
                KeyCode::Char('n') => self.next_finding(),
                KeyCode::Char('N') => self.prev_finding(),
                KeyCode::Char('t') => self.toggle_filter(),
                KeyCode::Char('?') => self.mode = Mode::Help,
                KeyCode::Char('R') => {
                    self.status = None;
                    self.mode = Mode::ReviewPick;
                }
                KeyCode::Char('c') => self.begin_comment(),
                KeyCode::Char('T') => self.open_threads(),
                KeyCode::Char('r') => self.begin_reply(),
                KeyCode::Char('x') => self.toggle_resolve(),
                KeyCode::Char('/') => {
                    self.mode = Mode::Search;
                    self.search_input.clear();
                }
                _ => {}
            },
            Mode::Search => match code {
                KeyCode::Enter => {
                    self.run_search();
                    self.mode = Mode::Normal;
                }
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Backspace => {
                    self.search_input.pop();
                }
                KeyCode::Char(c) => self.search_input.push(c),
                _ => {}
            },
            Mode::Help => match code {
                KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Esc => self.mode = Mode::Normal,
                _ => {}
            },
            Mode::ReviewPick => match code {
                KeyCode::Char('a') => self.begin_review("APPROVE"),
                KeyCode::Char('r') => self.begin_review("REQUEST_CHANGES"),
                KeyCode::Char('c') => self.begin_review("COMMENT"),
                KeyCode::Esc => self.mode = Mode::Normal,
                _ => {}
            },
            Mode::ReviewInput => match code {
                KeyCode::Enter => self.submit_review(),
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Backspace => {
                    self.review_input.pop();
                }
                KeyCode::Char(c) => self.review_input.push(c),
                _ => {}
            },
            Mode::CommentInput => match code {
                KeyCode::Enter => self.submit_comment(),
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Backspace => {
                    self.review_input.pop();
                }
                KeyCode::Char(c) => self.review_input.push(c),
                _ => {}
            },
            Mode::ReplyInput => match code {
                KeyCode::Enter => self.submit_reply(),
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Backspace => {
                    self.review_input.pop();
                }
                KeyCode::Char(c) => self.review_input.push(c),
                _ => {}
            },
        }
        false
    }

    /// Start an inline comment on the cursor line, if it's commentable.
    fn begin_comment(&mut self) {
        if !matches!(self.current_row(), Some(Row::File(_))) {
            return;
        }
        let cursor_t = self.targets.get(self.cursor as usize).copied().flatten();
        // (end, start) — start is the lower line of a same-side selection.
        let (end, start) = if let Some(anchor) = self.selection_anchor {
            let anchor_t = self.targets.get(anchor as usize).copied().flatten();
            match (anchor_t, cursor_t) {
                (Some(a), Some(b)) if a.side == b.side => {
                    if a.line <= b.line {
                        (b, Some(a))
                    } else {
                        (a, Some(b))
                    }
                }
                (Some(_), Some(_)) => {
                    self.status = Some("error: selection spans both sides".to_string());
                    return;
                }
                _ => {
                    self.status = Some("not a commentable selection".to_string());
                    return;
                }
            }
        } else {
            match cursor_t {
                Some(t) => (t, None),
                None => {
                    self.status = Some("not a commentable line".to_string());
                    return;
                }
            }
        };
        // Collapse to a single line if the range is one line.
        self.pending_comment = Some(end);
        self.pending_comment_start = start.filter(|s| s.line != end.line);
        self.selection_anchor = None;
        self.review_input.clear();
        self.status = None;
        self.mode = Mode::CommentInput;
    }

    /// Post the inline comment as a new review thread. Blocks the UI briefly.
    fn submit_comment(&mut self) {
        let body = self.review_input.clone();
        if body.trim().is_empty() {
            self.status = Some("error: comment needs a message".to_string());
            return;
        }
        let target = match self.pending_comment {
            Some(t) => t,
            None => {
                self.mode = Mode::Normal;
                return;
            }
        };
        let (start_line, start_side) = match self.pending_comment_start {
            Some(s) => (Some(s.line), Some(s.side)),
            None => (None, None),
        };
        let path = match self.selected() {
            Some(f) => f.path.clone(),
            None => {
                self.mode = Mode::Normal;
                return;
            }
        };

        let settings = marrow_core::config::load_settings();
        let github = marrow_core::github::GithubClient::new(
            marrow_core::config::resolve_github_token(&settings),
        );
        let parsed = match marrow_core::pr_parser::parse_pr_ref(&self.manifest.pr_url) {
            Ok(p) => p,
            Err(e) => {
                self.status = Some(format!("error: {e}"));
                self.mode = Mode::Normal;
                return;
            }
        };

        let result = block_on(async move {
            let pr_id = github
                .get_pull_request_id(&parsed.owner, &parsed.repo, parsed.number)
                .await?;
            github
                .create_review_thread(
                    &pr_id,
                    &body,
                    &path,
                    target.line,
                    target.side,
                    start_line,
                    start_side,
                )
                .await
        });
        let span = match start_line {
            Some(sl) => format!("{}:{sl}-{}", target.side, target.line),
            None => format!("{}:{}", target.side, target.line),
        };
        self.status = Some(match result {
            Ok(_) => format!("✓ comment added on {span}"),
            Err(e) => format!("error: {e}"),
        });
        self.mode = Mode::Normal;
    }

    /// Render the review threads into lines, plus a per-row thread index so the
    /// cursor knows which thread it's on.
    fn build_threads_view(&self) -> (Vec<Line<'static>>, Vec<Option<usize>>) {
        let mut lines: Vec<Line> = Vec::new();
        let mut rows: Vec<Option<usize>> = Vec::new();
        let threads = match &self.threads {
            Some(t) => t,
            None => {
                lines.push(Line::from(Span::styled(
                    "Press T to load review threads.",
                    Style::default().fg(Color::DarkGray),
                )));
                rows.push(None);
                return (lines, rows);
            }
        };
        if threads.is_empty() {
            lines.push(Line::from(Span::styled(
                "No review threads.",
                Style::default().fg(Color::DarkGray),
            )));
            rows.push(None);
            return (lines, rows);
        }
        for (ti, th) in threads.iter().enumerate() {
            let (state, color) = if th.is_resolved {
                ("resolved", Color::Green)
            } else {
                ("open", Color::Yellow)
            };
            let loc = th.line.map(|l| format!(":{l}")).unwrap_or_default();
            let outdated = if th.is_outdated { "  (outdated)" } else { "" };
            lines.push(Line::from(vec![
                Span::styled(format!("{state} "), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{}{}", th.path, loc), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(outdated.to_string(), Style::default().fg(Color::DarkGray)),
            ]));
            rows.push(Some(ti));
            for c in &th.comments {
                lines.push(Line::from(Span::styled(
                    format!("  @{}", c.author.login),
                    Style::default().fg(Color::Cyan),
                )));
                rows.push(Some(ti));
                for l in crate::wrap(&c.body, 76) {
                    lines.push(Line::from(format!("    {l}")));
                    rows.push(Some(ti));
                }
            }
            lines.push(Line::from(""));
            rows.push(None);
        }
        (lines, rows)
    }

    fn current_thread(&self) -> Option<usize> {
        self.thread_at_row.get(self.cursor as usize).copied().flatten()
    }

    /// Load threads (if needed) and switch to the threads view.
    fn open_threads(&mut self) {
        self.ensure_threads_loaded();
        if let Some(pos) = self.rows.iter().position(|r| matches!(r, Row::Threads)) {
            self.list_state.select(Some(pos));
            self.cursor = 0;
            self.cache_idx = None; // threads may have just loaded
        }
    }

    fn ensure_threads_loaded(&mut self) {
        if self.threads.is_some() {
            return;
        }
        let Some((github, pr)) = self.github_and_pr() else {
            return;
        };
        match block_on(github.get_review_threads(&pr.owner, &pr.repo, pr.number)) {
            Ok(t) => {
                self.status = Some(format!("loaded {} thread(s)", t.len()));
                self.threads = Some(t);
            }
            Err(e) => self.status = Some(format!("error: {e}")),
        }
    }

    /// Start a reply to the thread under the cursor — either in the Threads view
    /// or on an inline thread shown in a file's diff.
    fn begin_reply(&mut self) {
        let idx = match self.current_row() {
            Some(Row::Threads) => self.current_thread(),
            Some(Row::File(_)) => {
                self.diff_thread_at_row.get(self.cursor as usize).copied().flatten()
            }
            _ => None,
        };
        if let Some(idx) = idx {
            self.pending_thread = Some(idx);
            self.review_input.clear();
            self.status = None;
            self.mode = Mode::ReplyInput;
        }
    }

    fn submit_reply(&mut self) {
        let body = self.review_input.clone();
        if body.trim().is_empty() {
            self.status = Some("error: reply needs a message".to_string());
            return;
        }
        let comment_id = self
            .pending_thread
            .and_then(|i| self.threads.as_ref()?.get(i))
            .and_then(|th| th.comments.first())
            .map(|c| c.id.clone());
        let comment_id = match comment_id {
            Some(id) => id,
            None => {
                self.status = Some("error: thread has no comment to reply to".to_string());
                self.mode = Mode::Normal;
                return;
            }
        };
        let Some((github, pr)) = self.github_and_pr() else {
            self.mode = Mode::Normal;
            return;
        };
        let result = block_on(async move {
            let pr_id = github.get_pull_request_id(&pr.owner, &pr.repo, pr.number).await?;
            github.reply_to_review_thread(&pr_id, &comment_id, &body).await
        });
        self.status = Some(match result {
            Ok(_) => "✓ reply posted".to_string(),
            Err(e) => format!("error: {e}"),
        });
        self.mode = Mode::Normal;
    }

    /// Resolve / reopen the thread under the cursor (threads view only).
    fn toggle_resolve(&mut self) {
        if !matches!(self.current_row(), Some(Row::Threads)) {
            return;
        }
        let idx = match self.current_thread() {
            Some(i) => i,
            None => return,
        };
        let (thread_id, want) = match self.threads.as_ref().and_then(|t| t.get(idx)) {
            Some(th) => (th.id.clone(), !th.is_resolved),
            None => return,
        };
        let Some((github, _pr)) = self.github_and_pr() else {
            return;
        };
        match block_on(github.resolve_review_thread(&thread_id, want)) {
            Ok(now) => {
                if let Some(th) = self.threads.as_mut().and_then(|t| t.get_mut(idx)) {
                    th.is_resolved = now;
                }
                self.status = Some(if now { "✓ resolved".into() } else { "✓ reopened".into() });
                self.cache_idx = None; // re-render the new state
            }
            Err(e) => self.status = Some(format!("error: {e}")),
        }
    }

    fn github_and_pr(
        &mut self,
    ) -> Option<(marrow_core::github::GithubClient, marrow_core::pr_parser::ParsedPrRef)> {
        let settings = marrow_core::config::load_settings();
        let github = marrow_core::github::GithubClient::new(
            marrow_core::config::resolve_github_token(&settings),
        );
        match marrow_core::pr_parser::parse_pr_ref(&self.manifest.pr_url) {
            Ok(p) => Some((github, p)),
            Err(e) => {
                self.status = Some(format!("error: {e}"));
                None
            }
        }
    }

    fn begin_review(&mut self, event: &'static str) {
        self.review_event = event;
        self.review_input.clear();
        self.mode = Mode::ReviewInput;
    }

    /// Submit the pending review to GitHub. Blocks the UI briefly (the call is
    /// quick and the flow is modal); the result lands in the status line.
    fn submit_review(&mut self) {
        let event = self.review_event;
        let body = self.review_input.clone();
        if event == "REQUEST_CHANGES" && body.trim().is_empty() {
            // GitHub requires a body; stay in input so the user can add one.
            self.status = Some("error: request-changes needs a message".to_string());
            return;
        }

        let settings = marrow_core::config::load_settings();
        let github = marrow_core::github::GithubClient::new(
            marrow_core::config::resolve_github_token(&settings),
        );
        let parsed = match marrow_core::pr_parser::parse_pr_ref(&self.manifest.pr_url) {
            Ok(p) => p,
            Err(e) => {
                self.status = Some(format!("error: {e}"));
                self.mode = Mode::Normal;
                return;
            }
        };

        let result =
            block_on(github.submit_review(&parsed.owner, &parsed.repo, parsed.number, event, &body));
        self.status = Some(match result {
            Ok(state) => format!("✓ review submitted: {state}"),
            Err(e) => format!("error: {e}"),
        });
        self.mode = Mode::Normal;
    }

    fn ui(&mut self, f: &mut Frame) {
        self.sync_cache();
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
            .split(f.area());

        let mut header = vec![
            Span::styled(
                " marrow ",
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {} #{}", self.manifest.pr_title, self.manifest.pr_number)),
        ];
        if let Some(s) = &self.status {
            let color = if s.starts_with("error") { Color::Red } else { Color::Green };
            header.push(Span::styled(format!("   {s}"), Style::default().fg(color)));
        }
        f.render_widget(Paragraph::new(Line::from(header)), rows[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(40), Constraint::Min(1)])
            .split(rows[1]);

        self.render_sidebar(f, body[0]);
        self.render_diff(f, body[1]);

        let footer = match self.mode {
            Mode::Search => Paragraph::new(Line::from(vec![
                Span::styled("/", Style::default().fg(Color::Cyan)),
                Span::raw(self.search_input.clone()),
                Span::styled("▏", Style::default().fg(Color::Cyan)),
            ])),
            Mode::ReviewPick => Paragraph::new(
                " review:  a approve · r request-changes · c comment · Esc cancel ",
            )
            .style(Style::default().fg(Color::Yellow)),
            Mode::ReviewInput => Paragraph::new(Line::from(vec![
                Span::styled(
                    format!(" {} message: ", verb(self.review_event)),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(self.review_input.clone()),
                Span::styled("▏  (Enter submit · Esc cancel)", Style::default().fg(Color::DarkGray)),
            ])),
            Mode::CommentInput => {
                let loc = self
                    .pending_comment
                    .map(|t| format!(" comment {} L{}: ", t.side, t.line))
                    .unwrap_or_else(|| " comment: ".to_string());
                Paragraph::new(Line::from(vec![
                    Span::styled(loc, Style::default().fg(Color::Yellow)),
                    Span::raw(self.review_input.clone()),
                    Span::styled("▏  (Enter submit · Esc cancel)", Style::default().fg(Color::DarkGray)),
                ]))
            }
            Mode::ReplyInput => Paragraph::new(Line::from(vec![
                Span::styled(" reply: ", Style::default().fg(Color::Yellow)),
                Span::raw(self.review_input.clone()),
                Span::styled("▏  (Enter submit · Esc cancel)", Style::default().fg(Color::DarkGray)),
            ])),
            _ => {
                if let Some(anchor) = self.selection_anchor {
                    let n = anchor.abs_diff(self.cursor) + 1;
                    let hint = format!(" SELECT {n} lines · j/k extend · c comment · v/Esc cancel ");
                    Paragraph::new(hint).style(Style::default().fg(Color::Yellow))
                } else {
                    let hint = if matches!(self.current_row(), Some(Row::Threads)) {
                        " r reply · x resolve · T reload · ]/[ view · ? help · q quit "
                    } else {
                        " ]/[ file · n/N find · c comment · v select · R review · T threads · ? help · q quit "
                    };
                    Paragraph::new(hint).style(Style::default().fg(Color::DarkGray))
                }
            }
        };
        f.render_widget(footer, rows[2]);

        if self.mode == Mode::Help {
            render_help(f);
        }
    }

    fn render_sidebar(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .rows
            .iter()
            .map(|row| match row {
                Row::Overview => ListItem::new(Line::from(Span::styled(
                    "Overview",
                    Style::default().add_modifier(Modifier::BOLD),
                ))),
                Row::Threads => ListItem::new(Line::from(Span::styled(
                    "Threads",
                    Style::default().add_modifier(Modifier::BOLD),
                ))),
                Row::Group(label) => ListItem::new(Line::from(Span::styled(
                    format!("▸ {label}"),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ))),
                Row::File(i) => {
                    let file = self.files[*i];
                    // Risk is kept as the filename color (no HIGH/med/low text);
                    // churn shows additions/deletions.
                    let color = match file.risk_level.as_str() {
                        "high" => Color::Red,
                        "low" => Color::Green,
                        _ => Color::Yellow,
                    };
                    ListItem::new(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(short_path(&file.path, 22), Style::default().fg(color)),
                        Span::raw("  "),
                        Span::styled(format!("+{}", file.additions), Style::default().fg(Color::Green)),
                        Span::raw(" "),
                        Span::styled(format!("-{}", file.deletions), Style::default().fg(Color::Red)),
                    ]))
                }
            })
            .collect();

        let title = if self.filter_low { "Files (relevant)" } else { "Files" };
        let list = List::new(items)
            .block(Block::default().borders(Borders::RIGHT).title(title))
            .highlight_style(Style::default().bg(CURSOR_BG))
            .highlight_symbol("›");
        f.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_diff(&mut self, f: &mut Frame, area: Rect) {
        let is_overview = matches!(self.current_row(), Some(Row::Overview));
        let is_threads = matches!(self.current_row(), Some(Row::Threads));
        let title = if is_overview {
            " Overview ".to_string()
        } else if is_threads {
            " Threads ".to_string()
        } else {
            self.selected()
                .map(|file| format!(" {}  +{} -{} ", file.path, file.additions, file.deletions))
                .unwrap_or_else(|| " (no file) ".to_string())
        };

        // Keep the cursor within the viewport (content height = area minus the
        // block's title row).
        let total = self.diff_cache.len() as u16;
        let inner_h = area.height.saturating_sub(1).max(1);
        self.viewport_h = inner_h;
        if total > 0 {
            self.cursor = self.cursor.min(total - 1);
        }
        if self.cursor < self.view_top {
            self.view_top = self.cursor;
        } else if self.cursor >= self.view_top + inner_h {
            self.view_top = self.cursor + 1 - inner_h;
        }
        self.view_top = self.view_top.min(total.saturating_sub(inner_h));

        // Highlight the selection range and the cursor line (diff/threads only).
        let mut lines = self.diff_cache.clone();
        if !is_overview {
            if let Some(anchor) = self.selection_anchor {
                let (lo, hi) = (anchor.min(self.cursor), anchor.max(self.cursor));
                for r in lo..=hi {
                    if let Some(line) = lines.get_mut(r as usize) {
                        line.style = Style::default().bg(SELECT_BG);
                    }
                }
            }
            if let Some(line) = lines.get_mut(self.cursor as usize) {
                line.style = Style::default().bg(CURSOR_BG);
            }
        }

        let para = Paragraph::new(lines)
            .block(Block::default().title(title))
            .scroll((self.view_top, 0));
        f.render_widget(para, area);
    }
}

/// A rendered diff plus the row indices used for jump navigation.
#[derive(Default)]
struct Rendered {
    lines: Vec<Line<'static>>,
    hunk_rows: Vec<usize>,
    finding_rows: Vec<usize>,
    targets: Vec<Option<CommentTarget>>,
    /// Parallel to `lines`: the global `threads` index of the inline thread a row
    /// belongs to (the anchored code line and its 💬 block), else None.
    thread_at_row: Vec<Option<usize>>,
}

impl Rendered {
    fn message(msg: &str) -> Self {
        Rendered {
            lines: vec![Line::from(Span::styled(
                msg.to_string(),
                Style::default().fg(Color::DarkGray),
            ))],
            ..Default::default()
        }
    }
}

/// Background tint for inline review-comment blocks (a muted dark purple).
const COMMENT_BG: Color = Color::Rgb(52, 42, 75);
/// Cursor-line background, and the line-selection range background.
const CURSOR_BG: Color = Color::Rgb(45, 50, 60);
const SELECT_BG: Color = Color::Rgb(38, 48, 70);
/// Faint backgrounds for added / removed diff lines.
const ADD_BG: Color = Color::Rgb(20, 45, 28);
const DEL_BG: Color = Color::Rgb(52, 28, 30);

/// Render a file's diff as styled ratatui lines: syntect syntax highlighting on
/// the code, AI-highlight comments inline (`▸` above the line they start on), a
/// severity gutter bar (`▍`) on covered lines, and open review-thread comments
/// inline (`💬` below the line). Also records the row indices of hunk headers
/// and findings for jump navigation.
fn diff_lines_for(file: &FileDiff, threads: &[(usize, &ReviewThread)]) -> Rendered {
    // new-side line → highlights starting there, and lines covered by any (for
    // the gutter bar, keeping the highest severity).
    let mut starts: HashMap<u64, Vec<&Highlight>> = HashMap::new();
    let mut covered: HashMap<u64, Color> = HashMap::new();
    // new-side line → review threads anchored there (shown inline), each paired
    // with its global index in `App::threads` so inline rows can map back.
    let mut threads_at: HashMap<u64, Vec<(usize, &ReviewThread)>> = HashMap::new();
    for &(gi, t) in threads {
        if let Some(l) = t.line {
            threads_at.entry(l).or_default().push((gi, t));
        }
    }
    for h in &file.highlights {
        starts.entry(h.start_line).or_default().push(h);
        let c = sev_color(&h.severity);
        for ln in h.start_line..=h.end_line {
            covered
                .entry(ln)
                .and_modify(|cur| {
                    if sev_rank(c) > sev_rank(*cur) {
                        *cur = c;
                    }
                })
                .or_insert(c);
        }
    }

    let (ps, _) = crate::highlighter();
    let syntax = crate::syntax_for(ps, &file.path);

    let mut out: Vec<Line> = Vec::new();
    let mut hunk_rows: Vec<usize> = Vec::new();
    let mut finding_rows: Vec<usize> = Vec::new();
    let mut targets: Vec<Option<CommentTarget>> = Vec::new();
    let mut thread_at_row: Vec<Option<usize>> = Vec::new();
    let mut new_ln: Option<u64> = None;
    let mut old_ln: Option<u64> = None;
    for line in file.unified_diff.lines() {
        if line.starts_with("@@") {
            old_ln = hunk_old_start(line);
            new_ln = crate::hunk_new_start(line);
            hunk_rows.push(out.len());
            out.push(Line::from(Span::styled(line.to_string(), Style::default().fg(Color::Cyan))));
            targets.push(None);
            thread_at_row.push(None);
            continue;
        }

        let (marker, rest, on_new_side, marker_color) = match line.chars().next() {
            Some('+') => ('+', &line[1..], true, Color::Green),
            Some('-') => ('-', &line[1..], false, Color::Red),
            Some(' ') => (' ', &line[1..], true, Color::DarkGray),
            _ => {
                out.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::DarkGray),
                )));
                targets.push(None);
                thread_at_row.push(None);
                continue;
            }
        };

        let cur_new = if on_new_side { new_ln } else { None };

        // Annotation comment(s) above the line they start on.
        if let Some(ln) = cur_new {
            if let Some(hs) = starts.get(&ln) {
                for h in hs {
                    let c = sev_color(&h.severity);
                    let loc = if h.start_line == h.end_line {
                        format!("L{}", h.start_line)
                    } else {
                        format!("L{}-{}", h.start_line, h.end_line)
                    };
                    finding_rows.push(out.len());
                    out.push(Line::from(vec![
                        Span::styled(format!("▸ {loc} "), Style::default().fg(c).add_modifier(Modifier::BOLD)),
                        Span::styled(h.comment.clone(), Style::default().fg(c)),
                    ]));
                    targets.push(None);
                    thread_at_row.push(None);
                }
            }
        }

        // Faint full-width background tint so added/removed lines stand out from
        // context (the trailing pad fills the row; Paragraph truncates it).
        let line_bg = match marker {
            '+' => Some(ADD_BG),
            '-' => Some(DEL_BG),
            _ => None,
        };
        let mut spans: Vec<Span> = Vec::new();
        match cur_new.and_then(|ln| covered.get(&ln)) {
            Some(&c) => spans.push(Span::styled("▍", Style::default().fg(c))),
            None => spans.push(Span::raw(" ")),
        }
        spans.push(Span::styled(marker.to_string(), Style::default().fg(marker_color)));
        spans.extend(highlight_spans(rest, syntax));
        let code_row = out.len();
        match line_bg {
            Some(bg) => {
                spans.push(Span::raw(" ".repeat(200)));
                out.push(Line::from(spans).style(Style::default().bg(bg)));
            }
            None => out.push(Line::from(spans)),
        }

        // Comment target: removed lines map to the old side, everything else
        // (added/context) to the new side.
        let target = match marker {
            '-' => old_ln.map(|line| CommentTarget { line, side: "LEFT" }),
            _ => new_ln.map(|line| CommentTarget { line, side: "RIGHT" }),
        };
        targets.push(target);
        thread_at_row.push(None);

        // Open review-thread comments, shown inline below the line they're on,
        // on a tinted background with a magenta left bar so they stand out.
        if let Some(ln) = cur_new {
            if let Some(ths) = threads_at.get(&ln) {
                // The anchored code line replies to the first thread on it, so
                // `r` works with the cursor on the code or on the 💬 block.
                if let Some((gi, _)) = ths.first() {
                    thread_at_row[code_row] = Some(*gi);
                }
                let bg = Style::default().bg(COMMENT_BG);
                // Trailing pad so the line's background fills the row into a band
                // (a Line's bg only covers the cells it occupies).
                let bar = || Span::styled("▌ ", Style::default().fg(Color::Magenta));
                let pad = || Span::raw(" ".repeat(160));
                let mut push = |spans: Vec<Span<'static>>, ti: Option<usize>| {
                    let mut s = spans;
                    s.push(pad());
                    out.push(Line::from(s).style(bg));
                    targets.push(None);
                    thread_at_row.push(ti);
                };
                for (gi, th) in ths {
                    let gi = Some(*gi);
                    push(
                        vec![
                            bar(),
                            Span::styled(
                                "💬 thread",
                                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                            ),
                        ],
                        gi,
                    );
                    for c in &th.comments {
                        push(
                            vec![
                                bar(),
                                Span::styled(
                                    format!("@{}", c.author.login),
                                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                                ),
                            ],
                            gi,
                        );
                        for l in crate::wrap(&c.body, 72) {
                            push(
                                vec![
                                    bar(),
                                    Span::styled(
                                        format!("  {l}"),
                                        Style::default().fg(Color::White),
                                    ),
                                ],
                                gi,
                            );
                        }
                    }
                }
            }
        }

        // Advance the line counters: '+' new only, '-' old only, ' ' both.
        match marker {
            '+' => {
                if let Some(n) = new_ln.as_mut() {
                    *n += 1;
                }
            }
            '-' => {
                if let Some(o) = old_ln.as_mut() {
                    *o += 1;
                }
            }
            _ => {
                if let Some(n) = new_ln.as_mut() {
                    *n += 1;
                }
                if let Some(o) = old_ln.as_mut() {
                    *o += 1;
                }
            }
        }
    }
    Rendered { lines: out, hunk_rows, finding_rows, targets, thread_at_row }
}

/// Parse the old-side start line from a hunk header: `@@ -a,b +c,d @@` -> a.
fn hunk_old_start(header: &str) -> Option<u64> {
    header
        .split_whitespace()
        .find(|t| t.starts_with('-'))
        .and_then(|t| t.trim_start_matches('-').split(',').next())
        .and_then(|n| n.parse::<u64>().ok())
}

/// Syntect-highlight one code line into ratatui spans (foreground only).
fn highlight_spans(code: &str, syntax: &SyntaxReference) -> Vec<Span<'static>> {
    let (ps, theme) = crate::highlighter();
    let mut h = HighlightLines::new(syntax, theme);
    match h.highlight_line(code, ps) {
        Ok(ranges) => ranges
            .into_iter()
            .map(|(style, text)| {
                let fg = style.foreground;
                Span::styled(text.to_string(), Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b)))
            })
            .collect(),
        Err(_) => vec![Span::raw(code.to_string())],
    }
}

fn sev_color(sev: &str) -> Color {
    match sev {
        "high" | "warning" => Color::Red,
        "medium" => Color::Yellow,
        _ => Color::Cyan,
    }
}

fn sev_rank(c: Color) -> u8 {
    match c {
        Color::Red => 3,
        Color::Yellow => 2,
        _ => 1,
    }
}

fn risk_rank(risk: &str) -> u8 {
    match risk {
        "high" => 0,
        "low" => 2,
        _ => 1,
    }
}

/// Truncate a long path from the left for the narrow sidebar (char-safe).
fn short_path(path: &str, max: usize) -> String {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= max {
        return path.to_string();
    }
    let tail: String = chars[chars.len() - (max - 1)..].iter().collect();
    format!("…{tail}")
}

/// Run an async future to completion from the sync TUI loop. Valid because the
/// loop runs on tokio's multi-thread runtime (`#[tokio::main]`); block_in_place
/// lets us block this worker without stalling the executor.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn verb(event: &str) -> &'static str {
    match event {
        "APPROVE" => "approve",
        "REQUEST_CHANGES" => "request-changes",
        _ => "comment",
    }
}

/// A centered rectangle of the given size within `area`, clamped to fit.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Keybindings help overlay, centered over the screen.
fn render_help(f: &mut Frame) {
    let head = |s| Line::from(Span::styled(s, Style::default().add_modifier(Modifier::BOLD)));
    let lines = vec![
        head("Navigation"),
        Line::from("  j / k        line down / up"),
        Line::from("  ^d / ^u      half page down / up"),
        Line::from("  PgDn / PgUp  page down / up"),
        Line::from("  g / G        top / bottom"),
        Line::from("  ] / [        next / prev item"),
        Line::from("  } / {        next / prev hunk"),
        Line::from("  n / N        next / prev finding"),
        Line::from(""),
        head("View"),
        Line::from("  /            search in file"),
        Line::from("  t            filter low-risk"),
        Line::from(""),
        head("Actions"),
        Line::from("  c            comment on the cursor line"),
        Line::from("  v            select a line range, then c"),
        Line::from("  R            submit a review"),
        Line::from(""),
        head("Threads"),
        Line::from("  T            load / open threads"),
        Line::from("  r            reply (threads view or 💬 in diff)"),
        Line::from("  x            resolve / reopen"),
        Line::from(""),
        Line::from("  ?            toggle this help"),
        Line::from("  q / Esc      quit"),
    ];
    let area = centered_rect(40, lines.len() as u16 + 2, f.area());
    let popup =
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Keys "));
    f.render_widget(Clear, area);
    f.render_widget(popup, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use marrow_core::types::{ChangeGroup, CommentAuthor, ReviewComment};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn thread(resolved: bool, path: &str, line: u64, comment: &str) -> ReviewThread {
        ReviewThread {
            id: "t1".into(),
            is_resolved: resolved,
            is_outdated: false,
            path: path.into(),
            line: Some(line),
            original_line: Some(line),
            diff_hunk: String::new(),
            comments: vec![ReviewComment {
                id: "c1".into(),
                body: comment.into(),
                author: CommentAuthor { login: "alice".into(), avatar_url: String::new() },
                created_at: String::new(),
                updated_at: String::new(),
                url: String::new(),
                reactions: Vec::new(),
            }],
        }
    }

    fn lines_text(app: &App) -> String {
        app.diff_cache
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn select_threads(app: &mut App) {
        let pos = app.rows.iter().position(|r| matches!(r, Row::Threads)).unwrap();
        app.list_state.select(Some(pos));
        app.cache_idx = None;
        app.sync_cache();
    }

    fn press(app: &mut App, code: KeyCode) -> bool {
        app.on_key(code, KeyModifiers::NONE)
    }

    fn file(path: &str, risk: &str, diff: &str) -> FileDiff {
        FileDiff {
            path: path.to_string(),
            classification: "RELEVANT".to_string(),
            reason: String::new(),
            category: "Business Logic".to_string(),
            risk_level: risk.to_string(),
            diff_type: "modified".to_string(),
            base_content: String::new(),
            head_content: String::new(),
            unified_diff: diff.to_string(),
            additions: 1,
            deletions: 0,
            highlights: Vec::new(),
            hunk_scores: Vec::new(),
            diff_hash: String::new(),
        }
    }

    fn manifest() -> ReviewManifest {
        ReviewManifest {
            pr_title: "proof of concept".to_string(),
            pr_url: "https://github.com/cli/cli/pull/9000".to_string(),
            pr_number: 9000,
            base_ref: "trunk".to_string(),
            head_ref: "feature".to_string(),
            base_sha: "aaa".to_string(),
            head_sha: "bbb".to_string(),
            summary: String::new(),
            change_groups: Vec::new(),
            files: vec![
                file("pkg/low.go", "low", "@@ -1,1 +1,1 @@\n-a\n+b\n"),
                file("pkg/high.go", "high", "@@ -1,1 +1,2 @@\n a\n+b\n"),
            ],
        }
    }

    fn render_to_string(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| app.ui(f)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn high_risk_sorts_first() {
        let m = manifest();
        let app = App::new(&m);
        assert_eq!(app.files[0].path, "pkg/high.go");
        assert_eq!(app.files[1].path, "pkg/low.go");
    }

    #[test]
    fn renders_header_sidebar_and_diff() {
        let m = manifest();
        let mut app = App::new(&m);
        let out = render_to_string(&mut app, 100, 20);
        assert!(out.contains("marrow"), "header missing");
        assert!(out.contains("+1 -0"), "sidebar churn missing");
        assert!(out.contains("high.go"), "selected file path missing");
        assert!(out.contains("@@ -1,1 +1,2 @@"), "diff hunk missing");
        assert!(out.contains("q quit"), "footer missing");
    }

    #[test]
    fn renders_ai_annotation_and_gutter() {
        let mut m = manifest();
        // high.go's added line is new-side L2; flag it.
        m.files[1].highlights = vec![Highlight {
            start_line: 2,
            end_line: 2,
            severity: "high".to_string(),
            comment: "watch this bypass".to_string(),
        }];
        let mut app = App::new(&m);
        let out = render_to_string(&mut app, 100, 20);
        assert!(out.contains("▸ L2"), "annotation marker missing");
        assert!(out.contains("watch this bypass"), "annotation comment missing");
        assert!(out.contains('▍'), "severity gutter bar missing");
    }

    #[test]
    fn select_step_skips_headers_and_reaches_overview() {
        let m = manifest(); // high.go, low.go; no groups → [Overview, File, File]
        let mut app = App::new(&m);
        // Lands on the first (highest-risk) file.
        assert_eq!(app.selected().map(|f| f.path.as_str()), Some("pkg/high.go"));
        // Can't scroll above the top.
        app.cursor_up(5);
        assert_eq!(app.cursor, 0);
        // Forward to the next file, then clamp at the end.
        app.select_step(true);
        assert_eq!(app.selected().map(|f| f.path.as_str()), Some("pkg/low.go"));
        app.select_step(true);
        assert_eq!(app.selected().map(|f| f.path.as_str()), Some("pkg/low.go"));
        // Backward steps pass through the files, the Threads row, then the
        // Overview (no file selected).
        app.select_step(false); // low.go → high.go
        app.select_step(false); // high.go → Threads
        assert!(matches!(app.current_row(), Some(Row::Threads)));
        app.select_step(false); // Threads → Overview
        assert!(matches!(app.current_row(), Some(Row::Overview)));
        assert_eq!(app.selected_file_idx(), None);
    }

    #[test]
    fn grouping_and_low_risk_filter() {
        let mut m = manifest();
        m.change_groups = vec![ChangeGroup {
            label: "Core".into(),
            description: "core logic".into(),
            file_paths: vec!["pkg/high.go".into()],
        }];
        let mut app = App::new(&m);
        let groups = |a: &App| a.rows.iter().filter(|r| matches!(r, Row::Group(_))).count();
        let files = |a: &App| a.rows.iter().filter(|r| matches!(r, Row::File(_))).count();
        // Core (high.go) + Other (low.go).
        assert_eq!(groups(&app), 2);
        assert_eq!(files(&app), 2);
        // Filtering low-risk drops low.go and its now-empty "Other" section.
        app.toggle_filter();
        assert_eq!(files(&app), 1);
        assert_eq!(groups(&app), 1);
    }

    #[test]
    fn overview_shows_summary_and_counts() {
        let mut m = manifest();
        m.summary = "This PR adds a flag.".into();
        let mut app = App::new(&m);
        app.select_step(false); // first file → Threads
        app.select_step(false); // Threads → Overview
        let out = render_to_string(&mut app, 100, 20);
        assert!(out.contains("2 files"), "risk counts missing");
        assert!(out.contains("This PR adds a flag"), "summary missing");
    }

    #[test]
    fn hunk_and_finding_navigation() {
        let diff = "@@ -1,2 +1,3 @@\n line1\n+added_a\n line2\n@@ -10,2 +11,3 @@\n line10\n+added_b\n line11\n";
        let mut f = file("pkg/multi.go", "high", diff);
        f.highlights = vec![
            Highlight { start_line: 2, end_line: 2, severity: "high".into(), comment: "first".into() },
            Highlight { start_line: 12, end_line: 12, severity: "medium".into(), comment: "second".into() },
        ];
        let m = ReviewManifest { files: vec![f], ..manifest() };
        let mut app = App::new(&m);
        app.sync_cache();

        assert_eq!(app.hunk_rows.len(), 2, "two hunk headers");
        assert_eq!(app.finding_rows.len(), 2, "two findings");

        // From the top, `}` jumps to the second hunk header; `{` back to the first.
        app.cursor = 0;
        app.next_hunk();
        assert_eq!(app.cursor as usize, app.hunk_rows[1]);
        app.prev_hunk();
        assert_eq!(app.cursor as usize, app.hunk_rows[0]);

        // `n` cycles through findings.
        app.cursor = 0;
        app.next_finding();
        assert_eq!(app.cursor as usize, app.finding_rows[0]);
        app.next_finding();
        assert_eq!(app.cursor as usize, app.finding_rows[1]);
    }

    #[test]
    fn search_jumps_to_match() {
        let m = manifest();
        let mut app = App::new(&m);
        app.sync_cache();
        app.search_input = "b".to_string();
        app.cursor = 0;
        app.run_search();
        assert!(app.cursor > 0, "should jump to the line containing 'b'");
    }

    #[test]
    fn help_overlay_renders() {
        let m = manifest();
        let mut app = App::new(&m);
        app.mode = Mode::Help;
        let out = render_to_string(&mut app, 100, 30);
        assert!(out.contains("Keys"), "help title missing");
        assert!(out.contains("filter low-risk"), "help body missing");
    }

    #[test]
    fn empty_manifest_renders_overview() {
        let m = ReviewManifest { files: vec![], ..manifest() };
        let mut app = App::new(&m);
        let out = render_to_string(&mut app, 80, 20);
        assert!(out.contains("0 files"), "overview should show zero files");
    }

    #[test]
    fn review_flow_transitions() {
        let m = manifest();
        let mut app = App::new(&m);
        // R → verb picker → approve → message input.
        assert!(!press(&mut app, KeyCode::Char('R')));
        assert!(matches!(app.mode, Mode::ReviewPick));
        press(&mut app, KeyCode::Char('a'));
        assert!(matches!(app.mode, Mode::ReviewInput));
        assert_eq!(app.review_event, "APPROVE");
        // Typing accumulates the message.
        for c in "lgtm".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.review_input, "lgtm");
        // Esc backs out without submitting; q then quits.
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
        assert!(press(&mut app, KeyCode::Char('q')));
    }

    #[test]
    fn request_changes_requires_message() {
        let m = manifest();
        let mut app = App::new(&m);
        press(&mut app, KeyCode::Char('R'));
        press(&mut app, KeyCode::Char('r')); // REQUEST_CHANGES
        assert!(matches!(app.mode, Mode::ReviewInput));
        // Submitting empty hits the guard before any network call: stays in
        // input with an error status.
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::ReviewInput));
        assert!(app.status.as_deref().unwrap_or("").contains("needs a message"));
    }

    #[test]
    fn comment_targets_map_lines_and_sides() {
        let f = file("pkg/c.go", "high", "@@ -5,3 +5,3 @@\n ctx\n-removed\n+added\n");
        let m = ReviewManifest { files: vec![f], ..manifest() };
        let mut app = App::new(&m);
        app.sync_cache();
        assert_eq!(app.targets.len(), app.diff_cache.len(), "parallel to lines");
        assert!(app.targets[0].is_none(), "hunk header not commentable");
        let ctx = app.targets[1].unwrap(); // context → new side
        assert_eq!((ctx.side, ctx.line), ("RIGHT", 5));
        let removed = app.targets[2].unwrap(); // removed → old side
        assert_eq!((removed.side, removed.line), ("LEFT", 6));
        let added = app.targets[3].unwrap(); // added → new side
        assert_eq!((added.side, added.line), ("RIGHT", 6));
    }

    #[test]
    fn comment_flow_guards_non_commentable_and_empty() {
        let f = file("pkg/c.go", "high", "@@ -5,3 +5,3 @@\n ctx\n-removed\n+added\n");
        let m = ReviewManifest { files: vec![f], ..manifest() };
        let mut app = App::new(&m);
        app.sync_cache();
        // Cursor on the hunk header → not commentable.
        press(&mut app, KeyCode::Char('c'));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.status.as_deref().unwrap_or("").contains("not a commentable"));
        // Move to the context line → opens comment input with the right target.
        app.cursor_down(1);
        press(&mut app, KeyCode::Char('c'));
        assert!(matches!(app.mode, Mode::CommentInput));
        let t = app.pending_comment.unwrap();
        assert_eq!((t.side, t.line), ("RIGHT", 5));
        // Submitting empty hits the guard before any network call.
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::CommentInput));
        assert!(app.status.as_deref().unwrap_or("").contains("needs a message"));
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn threads_view_maps_rows_and_state() {
        let m = manifest();
        let mut app = App::new(&m);
        app.threads = Some(vec![thread(false, "pkg/a.go", 10, "please fix this")]);
        select_threads(&mut app);
        let text = lines_text(&app);
        assert!(text.contains("open"), "state shown");
        assert!(text.contains("pkg/a.go:10"), "path:line shown");
        assert!(text.contains("@alice"), "author shown");
        assert!(text.contains("please fix this"), "comment body shown");
        // The header row maps to thread 0; the cursor resolves to it.
        assert_eq!(app.thread_at_row.first().copied().flatten(), Some(0));
        app.cursor = 0;
        assert_eq!(app.current_thread(), Some(0));
    }

    #[test]
    fn reply_flow_guards_empty() {
        let m = manifest();
        let mut app = App::new(&m);
        app.threads = Some(vec![thread(false, "pkg/a.go", 10, "fix")]);
        select_threads(&mut app);
        app.cursor = 0;
        press(&mut app, KeyCode::Char('r'));
        assert!(matches!(app.mode, Mode::ReplyInput));
        assert_eq!(app.pending_thread, Some(0));
        // Submitting empty hits the guard before any network call.
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::ReplyInput));
        assert!(app.status.as_deref().unwrap_or("").contains("needs a message"));
    }

    #[test]
    fn open_thread_renders_inline_in_diff() {
        let m = manifest(); // default selects pkg/high.go (lines L1 ctx, L2 added)
        let mut app = App::new(&m);
        app.threads = Some(vec![thread(false, "pkg/high.go", 2, "needs a guard here")]);
        app.cache_idx = None;
        app.sync_cache();
        let text = lines_text(&app);
        assert!(text.contains('💬'), "inline comment marker");
        assert!(text.contains("@alice"), "author");
        assert!(text.contains("needs a guard here"), "comment body");
    }

    #[test]
    fn reply_from_diff_targets_inline_thread() {
        let m = manifest(); // default selects pkg/high.go (L1 ctx, L2 added)
        let mut app = App::new(&m);
        // Two threads; the one on high.go is at global index 1.
        app.threads = Some(vec![
            thread(false, "pkg/other.go", 3, "elsewhere"),
            thread(false, "pkg/high.go", 2, "needs a guard here"),
        ]);
        app.cache_idx = None;
        app.sync_cache();
        // The anchored code line (new-side L2) maps to thread index 1.
        let code_row = app
            .diff_thread_at_row
            .iter()
            .position(|t| *t == Some(1))
            .expect("inline thread row recorded");
        app.cursor = code_row as u16;
        press(&mut app, KeyCode::Char('r'));
        assert!(matches!(app.mode, Mode::ReplyInput));
        assert_eq!(app.pending_thread, Some(1));
        // A line with no inline thread does not start a reply.
        app.mode = Mode::Normal;
        app.cursor = 0; // hunk header
        press(&mut app, KeyCode::Char('r'));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn resolved_thread_not_shown_inline() {
        let m = manifest();
        let mut app = App::new(&m);
        app.threads = Some(vec![thread(true, "pkg/high.go", 2, "already handled")]);
        app.cache_idx = None;
        app.sync_cache();
        assert!(!lines_text(&app).contains("already handled"), "resolved threads stay out of the diff");
    }

    #[test]
    fn v_toggles_selection_and_sets_range() {
        // Three added lines → new-side L1, L2, L3 (all RIGHT).
        let f = file("pkg/m.go", "high", "@@ -1,0 +1,3 @@\n+a\n+b\n+c\n");
        let m = ReviewManifest { files: vec![f], ..manifest() };
        let mut app = App::new(&m);
        app.sync_cache();
        // Anchor on the first added line (row 1 = L1), extend to row 3 (L3).
        app.cursor = 1;
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(app.selection_anchor, Some(1));
        app.cursor = 3;
        press(&mut app, KeyCode::Char('c'));
        assert!(matches!(app.mode, Mode::CommentInput));
        let end = app.pending_comment.unwrap();
        let start = app.pending_comment_start.unwrap();
        assert_eq!((start.side, start.line), ("RIGHT", 1), "range start");
        assert_eq!((end.side, end.line), ("RIGHT", 3), "range end");
        assert_eq!(app.selection_anchor, None, "selection consumed");
    }

    #[test]
    fn v_pressed_twice_clears_selection() {
        let f = file("pkg/m.go", "high", "@@ -1,0 +1,3 @@\n+a\n+b\n+c\n");
        let m = ReviewManifest { files: vec![f], ..manifest() };
        let mut app = App::new(&m);
        app.sync_cache();
        app.cursor = 1;
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(app.selection_anchor, Some(1));
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(app.selection_anchor, None);
    }

    #[test]
    fn added_and_removed_lines_get_background() {
        let f = file("pkg/d.go", "high", "@@ -1,1 +1,2 @@\n ctx\n-gone\n+new\n");
        let m = ReviewManifest { files: vec![f], ..manifest() };
        let mut app = App::new(&m);
        app.sync_cache();
        // rows: 0 @@, 1 context, 2 removed, 3 added
        assert_eq!(app.diff_cache[1].style.bg, None, "context line has no tint");
        assert_eq!(app.diff_cache[2].style.bg, Some(DEL_BG), "removed line tinted");
        assert_eq!(app.diff_cache[3].style.bg, Some(ADD_BG), "added line tinted");
    }

    #[test]
    fn bulk_navigation() {
        let mut diff = String::from("@@ -1,0 +1,40 @@\n");
        for i in 0..40 {
            diff.push_str(&format!("+line{i}\n"));
        }
        let f = file("pkg/big.go", "high", &diff);
        let m = ReviewManifest { files: vec![f], ..manifest() };
        let mut app = App::new(&m);
        app.sync_cache();
        app.viewport_h = 20; // simulate a rendered viewport
        let last = app.diff_cache.len() as u16 - 1;

        app.on_key(KeyCode::Char('d'), KeyModifiers::CONTROL); // half page down
        assert_eq!(app.cursor, 10);
        press(&mut app, KeyCode::PageDown); // full page down
        assert_eq!(app.cursor, 30);
        press(&mut app, KeyCode::End); // bottom
        assert_eq!(app.cursor, last);
        app.on_key(KeyCode::Char('u'), KeyModifiers::CONTROL); // half page up
        assert_eq!(app.cursor, last - 10);
        press(&mut app, KeyCode::Home); // top
        assert_eq!(app.cursor, 0);
    }
}
