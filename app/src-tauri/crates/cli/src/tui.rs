//! Interactive TUI — Level 2, M1 skeleton (read-only review viewer).
//!
//! Read-only ⇒ the whole `ReviewManifest` is preloaded before we enter the
//! loop, so the event loop never needs to await. M1 covers: alt-screen in/out,
//! a relevance-ordered file sidebar, the selected file's diff (basic +/-
//! coloring), j/k scrolling, ]/[ file switching, and quit. Syntax highlighting
//! + AI annotations land in M2.

use std::collections::HashMap;
use std::io;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
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
}

struct App<'a> {
    manifest: &'a ReviewManifest,
    files: Vec<&'a FileDiff>,
    list_state: ListState,
    scroll: u16,
    /// Rendered diff for the selected file (built lazily, not per frame), plus
    /// the row indices of hunk headers and AI findings for jump navigation.
    diff_cache: Vec<Line<'static>>,
    hunk_rows: Vec<usize>,
    finding_rows: Vec<usize>,
    cache_idx: Option<usize>,
    mode: Mode,
    search_input: String,
}

impl<'a> App<'a> {
    fn new(manifest: &'a ReviewManifest) -> Self {
        // Relevance order: high risk first (matches `marrow diff`).
        let mut files: Vec<&FileDiff> = manifest.files.iter().collect();
        files.sort_by_key(|f| risk_rank(&f.risk_level));
        let mut list_state = ListState::default();
        if !files.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            manifest,
            files,
            list_state,
            scroll: 0,
            diff_cache: Vec::new(),
            hunk_rows: Vec::new(),
            finding_rows: Vec::new(),
            cache_idx: None,
            mode: Mode::Normal,
            search_input: String::new(),
        }
    }

    fn selected(&self) -> Option<&'a FileDiff> {
        self.list_state.selected().and_then(|i| self.files.get(i).copied())
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
        let rendered = match self.selected() {
            Some(file) if !file.unified_diff.is_empty() => diff_lines_for(file),
            Some(_) => Rendered::message("(no diff)"),
            None => Rendered::default(),
        };
        self.diff_cache = rendered.lines;
        self.hunk_rows = rendered.hunk_rows;
        self.finding_rows = rendered.finding_rows;
    }

    fn jump_next(rows: &[usize], from: u16) -> Option<u16> {
        rows.iter().copied().find(|&r| r as u16 > from).map(|r| r as u16)
    }

    fn jump_prev(rows: &[usize], from: u16) -> Option<u16> {
        rows.iter().copied().rev().find(|&r| (r as u16) < from).map(|r| r as u16)
    }

    fn next_hunk(&mut self) {
        if let Some(r) = Self::jump_next(&self.hunk_rows, self.scroll) {
            self.scroll = r;
        }
    }
    fn prev_hunk(&mut self) {
        if let Some(r) = Self::jump_prev(&self.hunk_rows, self.scroll) {
            self.scroll = r;
        }
    }
    fn next_finding(&mut self) {
        if let Some(r) = Self::jump_next(&self.finding_rows, self.scroll) {
            self.scroll = r;
        }
    }
    fn prev_finding(&mut self) {
        if let Some(r) = Self::jump_prev(&self.finding_rows, self.scroll) {
            self.scroll = r;
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
        let from = (self.scroll as usize + 1).min(self.diff_cache.len().saturating_sub(1));
        if let Some(i) = self.find_match(&self.search_input, from) {
            self.scroll = i as u16;
        }
    }

    fn next_file(&mut self) {
        if self.files.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some((i + 1).min(self.files.len() - 1)));
        self.scroll = 0;
    }

    fn prev_file(&mut self) {
        let i = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some(i.saturating_sub(1)));
        self.scroll = 0;
    }

    fn scroll_down(&mut self, n: u16) {
        let max = self.diff_line_count().saturating_sub(1);
        self.scroll = (self.scroll + n).min(max);
    }

    fn scroll_up(&mut self, n: u16) {
        self.scroll = self.scroll.saturating_sub(n);
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
            match self.mode {
                Mode::Normal => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('j') | KeyCode::Down => self.scroll_down(1),
                    KeyCode::Char('k') | KeyCode::Up => self.scroll_up(1),
                    KeyCode::Char('g') => self.scroll = 0,
                    KeyCode::Char('G') => self.scroll = self.diff_line_count().saturating_sub(1),
                    KeyCode::Char(']') | KeyCode::Tab => self.next_file(),
                    KeyCode::Char('[') | KeyCode::BackTab => self.prev_file(),
                    KeyCode::Char('}') => self.next_hunk(),
                    KeyCode::Char('{') => self.prev_hunk(),
                    KeyCode::Char('n') => self.next_finding(),
                    KeyCode::Char('N') => self.prev_finding(),
                    KeyCode::Char('/') => {
                        self.mode = Mode::Search;
                        self.search_input.clear();
                    }
                    _ => {}
                },
                Mode::Search => match key.code {
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
            }
        }
        Ok(())
    }

    fn ui(&mut self, f: &mut Frame) {
        self.sync_cache();
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
            .split(f.area());

        let header = Line::from(vec![
            Span::styled(
                " marrow ",
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {} #{}", self.manifest.pr_title, self.manifest.pr_number)),
        ]);
        f.render_widget(Paragraph::new(header), rows[0]);

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
            Mode::Normal => Paragraph::new(
                " j/k scroll · ]/[ file · }/{ hunk · n/N finding · / search · q quit ",
            )
            .style(Style::default().fg(Color::DarkGray)),
        };
        f.render_widget(footer, rows[2]);
    }

    fn render_sidebar(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .files
            .iter()
            .map(|file| {
                let (label, color) = match file.risk_level.as_str() {
                    "high" => ("HIGH", Color::Red),
                    "low" => ("low ", Color::Green),
                    _ => ("med ", Color::Yellow),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(label, Style::default().fg(color).add_modifier(Modifier::BOLD)),
                    Span::raw(" "),
                    Span::raw(short_path(&file.path)),
                ]))
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::RIGHT).title("Files"))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("›");
        f.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_diff(&self, f: &mut Frame, area: Rect) {
        let title = self
            .selected()
            .map(|file| format!(" {}  +{} -{} ", file.path, file.additions, file.deletions))
            .unwrap_or_else(|| " (no file) ".to_string());

        let para = Paragraph::new(self.diff_cache.clone())
            .block(Block::default().title(title))
            .scroll((self.scroll, 0));
        f.render_widget(para, area);
    }
}

/// A rendered diff plus the row indices used for jump navigation.
#[derive(Default)]
struct Rendered {
    lines: Vec<Line<'static>>,
    hunk_rows: Vec<usize>,
    finding_rows: Vec<usize>,
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
    let mut new_ln: Option<u64> = None;
    for line in file.unified_diff.lines() {
        if line.starts_with("@@") {
            new_ln = crate::hunk_new_start(line);
            hunk_rows.push(out.len());
            out.push(Line::from(Span::styled(line.to_string(), Style::default().fg(Color::Cyan))));
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

        if on_new_side {
            if let Some(n) = new_ln.as_mut() {
                *n += 1;
            }
        }
    }
    Rendered { lines: out, hunk_rows, finding_rows }
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn scroll_and_file_switch_are_clamped() {
        let m = manifest();
        let mut app = App::new(&m);
        // Can't scroll above the top.
        app.scroll_up(5);
        assert_eq!(app.scroll, 0);
        // Switching files resets scroll and clamps at the ends.
        app.scroll_down(2);
        app.next_file();
        assert_eq!(app.scroll, 0);
        app.next_file(); // already last → stays last
        assert_eq!(app.list_state.selected(), Some(1));
        app.prev_file();
        app.prev_file(); // already first → stays first
        assert_eq!(app.list_state.selected(), Some(0));
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
        app.scroll = 0;
        app.next_hunk();
        assert_eq!(app.scroll as usize, app.hunk_rows[1]);
        app.prev_hunk();
        assert_eq!(app.scroll as usize, app.hunk_rows[0]);

        // `n` cycles through findings.
        app.scroll = 0;
        app.next_finding();
        assert_eq!(app.scroll as usize, app.finding_rows[0]);
        app.next_finding();
        assert_eq!(app.scroll as usize, app.finding_rows[1]);
    }

    #[test]
    fn search_jumps_to_match() {
        let m = manifest();
        let mut app = App::new(&m);
        app.sync_cache();
        app.search_input = "b".to_string();
        app.scroll = 0;
        app.run_search();
        assert!(app.scroll > 0, "should jump to the line containing 'b'");
    }
}
