//! Interactive TUI — Level 2, M1 skeleton (read-only review viewer).
//!
//! Read-only ⇒ the whole `ReviewManifest` is preloaded before we enter the
//! loop, so the event loop never needs to await. M1 covers: alt-screen in/out,
//! a relevance-ordered file sidebar, the selected file's diff (basic +/-
//! coloring), j/k scrolling, ]/[ file switching, and quit. Syntax highlighting
//! + AI annotations land in M2.

use std::collections::{HashMap, HashSet};
use std::io;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use syntect::easy::HighlightLines;
use syntect::parsing::SyntaxReference;

use marrow_core::types::{FileDiff, Highlight, ReviewManifest};

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
    /// The line being commented on while in CommentInput mode.
    pending_comment: Option<CommentTarget>,
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
        let mut rows = vec![Row::Overview];

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
        let rendered = match self.current_row() {
            Some(Row::Overview) => self.build_overview(),
            Some(Row::File(i)) => {
                let file = self.files[*i];
                if file.unified_diff.is_empty() {
                    Rendered::message("(no diff)")
                } else {
                    diff_lines_for(file)
                }
            }
            _ => Rendered::default(),
        };
        self.diff_cache = rendered.lines;
        self.hunk_rows = rendered.hunk_rows;
        self.finding_rows = rendered.finding_rows;
        self.targets = rendered.targets;
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

    fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        loop {
            terminal.draw(|f| self.ui(f))?;
            // Blocking read — also delivers resize events, which just redraw.
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if self.on_key(key.code) {
                break;
            }
        }
        Ok(())
    }

    /// Handle one key press. Returns true when the app should quit.
    fn on_key(&mut self, code: KeyCode) -> bool {
        match self.mode {
            Mode::Normal => match code {
                KeyCode::Char('q') | KeyCode::Esc => return true,
                KeyCode::Char('j') | KeyCode::Down => self.cursor_down(1),
                KeyCode::Char('k') | KeyCode::Up => self.cursor_up(1),
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
        }
        false
    }

    /// Start an inline comment on the cursor line, if it's commentable.
    fn begin_comment(&mut self) {
        if !matches!(self.current_row(), Some(Row::File(_))) {
            return;
        }
        match self.targets.get(self.cursor as usize).copied().flatten() {
            Some(target) => {
                self.pending_comment = Some(target);
                self.review_input.clear();
                self.status = None;
                self.mode = Mode::CommentInput;
            }
            None => self.status = Some("not a commentable line".to_string()),
        }
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
                .create_review_thread(&pr_id, &body, &path, target.line, target.side, None, None)
                .await
        });
        self.status = Some(match result {
            Ok(_) => format!("✓ comment added on {}:{}", target.side, target.line),
            Err(e) => format!("error: {e}"),
        });
        self.mode = Mode::Normal;
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
            _ => Paragraph::new(
                " ]/[ file · }/{ hunk · n/N find · c comment · R review · / search · t filter · ? help · q quit ",
            )
            .style(Style::default().fg(Color::DarkGray)),
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
                Row::Group(label) => ListItem::new(Line::from(Span::styled(
                    format!("▸ {label}"),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ))),
                Row::File(i) => {
                    let file = self.files[*i];
                    let (label, color) = match file.risk_level.as_str() {
                        "high" => ("HIGH", Color::Red),
                        "low" => ("low ", Color::Green),
                        _ => ("med ", Color::Yellow),
                    };
                    ListItem::new(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(label, Style::default().fg(color).add_modifier(Modifier::BOLD)),
                        Span::raw(" "),
                        Span::raw(short_path(&file.path)),
                    ]))
                }
            })
            .collect();

        let title = if self.filter_low { "Files (relevant)" } else { "Files" };
        let list = List::new(items)
            .block(Block::default().borders(Borders::RIGHT).title(title))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("›");
        f.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_diff(&mut self, f: &mut Frame, area: Rect) {
        let is_overview = matches!(self.current_row(), Some(Row::Overview));
        let is_file = matches!(self.current_row(), Some(Row::File(_)));
        let title = if is_overview {
            " Overview ".to_string()
        } else {
            self.selected()
                .map(|file| format!(" {}  +{} -{} ", file.path, file.additions, file.deletions))
                .unwrap_or_else(|| " (no file) ".to_string())
        };

        // Keep the cursor within the viewport (content height = area minus the
        // block's title row).
        let total = self.diff_cache.len() as u16;
        let inner_h = area.height.saturating_sub(1).max(1);
        if total > 0 {
            self.cursor = self.cursor.min(total - 1);
        }
        if self.cursor < self.view_top {
            self.view_top = self.cursor;
        } else if self.cursor >= self.view_top + inner_h {
            self.view_top = self.cursor + 1 - inner_h;
        }
        self.view_top = self.view_top.min(total.saturating_sub(inner_h));

        // Highlight the cursor line in the diff (not the overview prose).
        let mut lines = self.diff_cache.clone();
        if is_file {
            if let Some(line) = lines.get_mut(self.cursor as usize) {
                line.style = Style::default().bg(Color::Rgb(45, 50, 60));
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

/// Render a file's diff as styled ratatui lines: syntect syntax highlighting on
/// the code, AI-highlight comments inline (`▸` above the line they start on),
/// and a severity gutter bar (`▍`) on covered lines. Also records the row
/// indices of hunk headers and findings for jump navigation. Reuses the Level 1
/// syntect + hunk-parsing helpers from the CLI module.
fn diff_lines_for(file: &FileDiff) -> Rendered {
    // new-side line → highlights starting there, and lines covered by any (for
    // the gutter bar, keeping the highest severity).
    let mut starts: HashMap<u64, Vec<&Highlight>> = HashMap::new();
    let mut covered: HashMap<u64, Color> = HashMap::new();
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
    let mut new_ln: Option<u64> = None;
    let mut old_ln: Option<u64> = None;
    for line in file.unified_diff.lines() {
        if line.starts_with("@@") {
            old_ln = hunk_old_start(line);
            new_ln = crate::hunk_new_start(line);
            hunk_rows.push(out.len());
            out.push(Line::from(Span::styled(line.to_string(), Style::default().fg(Color::Cyan))));
            targets.push(None);
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
                }
            }
        }

        let mut spans: Vec<Span> = Vec::new();
        match cur_new.and_then(|ln| covered.get(&ln)) {
            Some(&c) => spans.push(Span::styled("▍", Style::default().fg(c))),
            None => spans.push(Span::raw(" ")),
        }
        spans.push(Span::styled(marker.to_string(), Style::default().fg(marker_color)));
        spans.extend(highlight_spans(rest, syntax));
        out.push(Line::from(spans));

        // Comment target: removed lines map to the old side, everything else
        // (added/context) to the new side.
        let target = match marker {
            '-' => old_ln.map(|line| CommentTarget { line, side: "LEFT" }),
            _ => new_ln.map(|line| CommentTarget { line, side: "RIGHT" }),
        };
        targets.push(target);

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
    Rendered { lines: out, hunk_rows, finding_rows, targets }
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
fn short_path(path: &str) -> String {
    const MAX: usize = 34;
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= MAX {
        return path.to_string();
    }
    let tail: String = chars[chars.len() - (MAX - 1)..].iter().collect();
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
        Line::from("  j / k        scroll"),
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
        Line::from("  R            submit a review"),
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
    use marrow_core::types::ChangeGroup;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

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
        assert!(out.contains("HIGH"), "risk badge missing");
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
        // Backward past the first file reaches the overview (no file selected).
        app.select_step(false);
        app.select_step(false);
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
        app.select_step(false); // first file → overview
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
        assert!(!app.on_key(KeyCode::Char('R')));
        assert!(matches!(app.mode, Mode::ReviewPick));
        app.on_key(KeyCode::Char('a'));
        assert!(matches!(app.mode, Mode::ReviewInput));
        assert_eq!(app.review_event, "APPROVE");
        // Typing accumulates the message.
        for c in "lgtm".chars() {
            app.on_key(KeyCode::Char(c));
        }
        assert_eq!(app.review_input, "lgtm");
        // Esc backs out without submitting; q then quits.
        app.on_key(KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.on_key(KeyCode::Char('q')));
    }

    #[test]
    fn request_changes_requires_message() {
        let m = manifest();
        let mut app = App::new(&m);
        app.on_key(KeyCode::Char('R'));
        app.on_key(KeyCode::Char('r')); // REQUEST_CHANGES
        assert!(matches!(app.mode, Mode::ReviewInput));
        // Submitting empty hits the guard before any network call: stays in
        // input with an error status.
        app.on_key(KeyCode::Enter);
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
        app.on_key(KeyCode::Char('c'));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.status.as_deref().unwrap_or("").contains("not a commentable"));
        // Move to the context line → opens comment input with the right target.
        app.cursor_down(1);
        app.on_key(KeyCode::Char('c'));
        assert!(matches!(app.mode, Mode::CommentInput));
        let t = app.pending_comment.unwrap();
        assert_eq!((t.side, t.line), ("RIGHT", 5));
        // Submitting empty hits the guard before any network call.
        app.on_key(KeyCode::Enter);
        assert!(matches!(app.mode, Mode::CommentInput));
        assert!(app.status.as_deref().unwrap_or("").contains("needs a message"));
        app.on_key(KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Normal));
    }
}
