//! Interactive TUI — Level 2, M1 skeleton (read-only review viewer).
//!
//! Read-only ⇒ the whole `ReviewManifest` is preloaded before we enter the
//! loop, so the event loop never needs to await. M1 covers: alt-screen in/out,
//! a relevance-ordered file sidebar, the selected file's diff (basic +/-
//! coloring), j/k scrolling, ]/[ file switching, and quit. Syntax highlighting
//! + AI annotations land in M2.

use std::io;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use marrow_core::types::{FileDiff, ReviewManifest};

/// Enter the alternate screen, run the viewer, and always restore the terminal.
pub fn run(manifest: &ReviewManifest) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::new(manifest);
    let res = app.run(&mut terminal);
    ratatui::restore();
    res
}

struct App<'a> {
    manifest: &'a ReviewManifest,
    files: Vec<&'a FileDiff>,
    list_state: ListState,
    scroll: u16,
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
        Self { manifest, files, list_state, scroll: 0 }
    }

    fn selected(&self) -> Option<&FileDiff> {
        self.list_state.selected().and_then(|i| self.files.get(i).copied())
    }

    fn diff_line_count(&self) -> u16 {
        self.selected()
            .map(|f| f.unified_diff.lines().count().min(u16::MAX as usize) as u16)
            .unwrap_or(0)
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
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('j') | KeyCode::Down => self.scroll_down(1),
                KeyCode::Char('k') | KeyCode::Up => self.scroll_up(1),
                KeyCode::Char('g') => self.scroll = 0,
                KeyCode::Char('G') => self.scroll = self.diff_line_count().saturating_sub(1),
                KeyCode::Char(']') | KeyCode::Tab => self.next_file(),
                KeyCode::Char('[') | KeyCode::BackTab => self.prev_file(),
                _ => {}
            }
        }
        Ok(())
    }

    fn ui(&mut self, f: &mut Frame) {
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

        let footer = Paragraph::new(" j/k scroll · g/G top/bottom · ]/[ file · q quit ")
            .style(Style::default().fg(Color::DarkGray));
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

        let lines: Vec<Line> = match self.selected() {
            Some(file) if !file.unified_diff.is_empty() => {
                file.unified_diff.lines().map(diff_line).collect()
            }
            Some(_) => vec![Line::from(Span::styled(
                "(no diff)",
                Style::default().fg(Color::DarkGray),
            ))],
            None => Vec::new(),
        };

        let para = Paragraph::new(lines)
            .block(Block::default().title(title))
            .scroll((self.scroll, 0));
        f.render_widget(para, area);
    }
}

/// Basic +/- diff coloring for M1 (syntect highlighting comes in M2).
fn diff_line(line: &str) -> Line<'static> {
    let style = if line.starts_with("@@") {
        Style::default().fg(Color::Cyan)
    } else if line.starts_with('+') {
        Style::default().fg(Color::Green)
    } else if line.starts_with('-') {
        Style::default().fg(Color::Red)
    } else {
        Style::default()
    };
    Line::from(Span::styled(line.to_string(), style))
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
}
