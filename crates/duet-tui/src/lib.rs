//! A ratatui TUI for viewing a run's conversation: a scrollable, attributed
//! timeline with a phase-aware header and a findings panel — the rich form of
//! "see the conversation like Claude Code".
//!
//! The same [`AgentEvent`] stream that feeds the headless renderer feeds this,
//! so a live mode (pushing events over a channel while the agents run) is a
//! drop-in extension. For now it powers `duet tui` (replay a saved run). The
//! `snapshot` function renders one frame to a string via ratatui's `TestBackend`
//! — used by tests and `--snapshot`, so the TUI is verifiable without a tty.

mod shell;
mod theme;
pub use shell::{run_shell, ShellAction, ShellController};

use duet_core::events::{parse_claude_line, parse_codex_line, AgentEvent};
use duet_core::render::{model_of_filename, Model};
use duet_core::report::{FindingRow, Sys, UiMsg};
use anyhow::{anyhow, Result};
use ratatui::{
    backend::{CrosstermBackend, TestBackend},
    buffer::Buffer,
    crossterm::{
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame, Terminal,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Duration;

pub enum Row {
    Phase(String),
    Note(String),
    /// An engine system line (note / ✓ ok / ! warn), colored by kind.
    Sys(Sys, String),
    /// One inline review finding, colored by severity.
    Finding(FindingRow),
    /// A convergence verdict — `true` = in harmony (ok), `false` = still tuning (warn).
    Verdict(bool, String),
    Event(Model, AgentEvent),
}

pub struct App {
    pub title: String,
    pub status: String,
    pub rows: Vec<Row>,
    pub findings: Vec<FindingRow>,
    pub scroll: usize,
    pub follow: bool,
}

#[derive(Deserialize, Default)]
struct Findings {
    #[serde(default)]
    verdict: String,
    #[serde(default)]
    findings: Vec<Finding>,
}
#[derive(Deserialize)]
struct Finding {
    #[serde(default)]
    severity: String,
    #[serde(default)]
    file: String,
    #[serde(default)]
    line: i64,
    #[serde(default)]
    issue: String,
}

impl App {
    /// Build the viewer state from a repo's `.duet` directory.
    pub fn from_duet(dir: &Path) -> Result<App> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|_| anyhow!("no .duet/ at {}", dir.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|f| {
                let n = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                n.starts_with("stream-") && n.ends_with(".jsonl")
            })
            .collect();
        files.sort();
        if files.is_empty() {
            return Err(anyhow!("no saved conversation streams in {}", dir.display()));
        }

        let mut rows = Vec::new();
        for f in &files {
            let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
            let Some(model) = model_of_filename(&name) else { continue };
            rows.push(Row::Phase(phase_label(&name)));
            let body = std::fs::read_to_string(f).unwrap_or_default();
            let parse: fn(&str) -> Vec<AgentEvent> = match model {
                Model::Claude => parse_claude_line,
                Model::Codex => parse_codex_line,
                Model::Local => duet_core::events::parse_local_line,
            };
            for line in body.lines() {
                for ev in parse(line) {
                    rows.push(Row::Event(model, ev));
                }
            }
        }

        // Highest-numbered findings file drives the findings panel + status.
        let (mut findings, mut status) = (Vec::new(), String::from("done"));
        let mut findings_files: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|f| {
                let n = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                n.starts_with("findings-") && n.ends_with(".json")
            })
            .collect();
        findings_files.sort();
        if let Some(fp) = findings_files.last() {
            if let Ok(parsed) = serde_json::from_str::<Findings>(&std::fs::read_to_string(fp).unwrap_or_default()) {
                status = if parsed.verdict.is_empty() { status } else { parsed.verdict };
                findings = parsed
                    .findings
                    .into_iter()
                    .map(|f| FindingRow { sev: f.severity, file: f.file, line: f.line, issue: f.issue })
                    .collect();
            }
        }

        let title = dir
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "duet".into());

        Ok(App { title, status, rows, findings, scroll: 0, follow: true })
    }

    fn scroll_up(&mut self, n: usize) {
        self.follow = false;
        self.scroll = self.scroll.saturating_sub(n);
    }
    fn scroll_down(&mut self, n: usize) {
        self.follow = false;
        self.scroll = self.scroll.saturating_add(n);
    }

    /// An empty viewer for a live run.
    pub fn live(title: String) -> App {
        App { title, status: "running".into(), rows: Vec::new(), findings: Vec::new(), scroll: 0, follow: true }
    }

    /// Fold one engine message into the view.
    pub fn apply(&mut self, msg: UiMsg) {
        match msg {
            UiMsg::Phase(p) => self.rows.push(Row::Phase(p)),
            UiMsg::Sys(kind, t) => self.rows.push(Row::Sys(kind, t)),
            UiMsg::Say(m, t) => self.rows.push(Row::Note(format!("[{}] {t}", m.label()))),
            UiMsg::Event(m, ev) => self.rows.push(Row::Event(m, ev)),
            UiMsg::Findings(v) => self.findings = v,
            UiMsg::Status(s) => self.status = s,
            UiMsg::Done(c) => self.status = if c == 0 { "done ✓".into() } else { format!("exit {c}") },
        }
    }
}

fn phase_label(filename: &str) -> String {
    // stream-02-redteam-codex.jsonl  ->  "Plan red-team · codex"
    let parts: Vec<&str> = filename.trim_end_matches(".jsonl").split('-').collect();
    let kind = parts.get(2).copied().unwrap_or("");
    let model = parts.get(3).copied().unwrap_or("");
    let pretty = match kind {
        "plan" => "Plan",
        "redteam" => "Plan red-team",
        "planrev" => "Plan revise",
        "build" => "Build",
        "review" => "Review",
        other => other,
    };
    format!("{pretty} · {model}")
}

fn flat(s: &str, max: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > max {
        one.chars().take(max).collect::<String>() + "…"
    } else {
        one
    }
}

fn model_color(m: Model) -> Color {
    match m {
        Model::Claude => theme::claude(),
        Model::Codex => theme::codex(),
        Model::Local => theme::local(),
    }
}

// ─────────────────── Material / Nerd-Font icons (file & tool) ────────────────
// Set DUET_NO_ICONS=1 to fall back to plain glyphs (no Nerd Font required).

fn icons_on() -> bool {
    std::env::var_os("DUET_NO_ICONS").is_none()
}

fn tool_icon(name: &str) -> &'static str {
    if !icons_on() {
        return "⚙";
    }
    match name {
        "Edit" | "MultiEdit" => "\u{f044}",          // edit
        "Write" | "NotebookEdit" => "\u{f0219}",     // file-plus
        "Read" => "\u{f06e}",                        // eye
        "Bash" => "\u{f120}",                        // terminal
        "Grep" | "Glob" | "Search" => "\u{f002}",    // magnifier
        "WebSearch" | "WebFetch" => "\u{f0ac}",      // globe
        "Task" | "Agent" => "\u{f0c0}",              // people
        _ => "\u{f013}",                             // gear
    }
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn file_icon(path: &str) -> &'static str {
    if !icons_on() {
        return "·";
    }
    let ext = basename(path).rsplit('.').next().unwrap_or("");
    match ext {
        "rs" => "\u{e7a8}",
        "py" => "\u{e606}",
        "md" | "markdown" => "\u{e609}",
        "json" | "jsonl" => "\u{e60b}",
        "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" => "\u{e615}",
        "js" | "mjs" | "cjs" => "\u{e74e}",
        "ts" | "tsx" => "\u{e628}",
        "go" => "\u{e627}",
        "sh" | "bash" | "zsh" => "\u{e795}",
        "html" | "htm" => "\u{e736}",
        "css" | "scss" => "\u{e749}",
        "c" | "h" => "\u{e61e}",
        "cpp" | "cc" | "cxx" | "hpp" => "\u{e61d}",
        "java" => "\u{e738}",
        "rb" => "\u{e791}",
        "txt" | "log" => "\u{f15c}",
        "diff" | "patch" => "\u{f440}",
        "" => "\u{e5ff}", // dir-ish / no extension
        _ => "\u{f15b}",  // generic file
    }
}

fn file_color(path: &str) -> Color {
    theme::file(basename(path).rsplit('.').next().unwrap_or(""))
}

/// Pull a `file_path` out of a tool-call input JSON, if present.
fn file_path_of(input: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(input).ok()?;
    for key in ["file_path", "path", "notebook_path"] {
        if let Some(p) = v.get(key).and_then(|x| x.as_str()) {
            return Some(p.to_string());
        }
    }
    None
}

fn file_span(path: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(format!("{} ", file_icon(path)), Style::default().fg(file_color(path))),
        Span::raw(basename(path).to_string()),
    ]
}

/// Render one scrollback row to a styled line. `width` lets phase dividers draw
/// a rule that fills the pane (the design-system PhaseDivider).
fn row_line(row: &Row, width: u16) -> Line<'static> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    match row {
        Row::Phase(p) => {
            // "── {label} ───────…" — a muted, bold rule that fills the row.
            let prefix = format!("── {p} ");
            let fill = (width as usize).saturating_sub(prefix.chars().count()).max(2);
            Line::from(vec![
                Span::styled(prefix, Style::default().fg(theme::muted()).add_modifier(Modifier::BOLD)),
                Span::styled("─".repeat(fill), Style::default().fg(theme::border())),
            ])
        }
        Row::Note(t) => Line::from(Span::styled(t.clone(), dim)),
        Row::Sys(kind, t) => {
            let (glyph, color) = match kind {
                Sys::Note => ("  ", theme::muted()),
                Sys::Ok => ("✓ ", theme::ok()),
                Sys::Warn => ("! ", theme::warn()),
            };
            Line::from(Span::styled(format!("{glyph}{t}"), Style::default().fg(color)))
        }
        Row::Finding(f) => finding_line(f),
        Row::Verdict(converged, t) => {
            let color = if *converged { theme::ok() } else { theme::warn() };
            Line::from(Span::styled(t.clone(), Style::default().fg(color).add_modifier(Modifier::BOLD)))
        }
        Row::Event(m, ev) => {
            let c = model_color(*m);
            let cstyle = Style::default().fg(c);
            let gutter = Span::styled("┃ ", cstyle);
            let label = Span::styled(format!("{:<6} ", m.label()), cstyle);
            let mut spans = vec![gutter, label];
            match ev {
                AgentEvent::Message(t) => spans.push(Span::styled(flat(t, 4000), Style::default().fg(theme::text()))),
                AgentEvent::Reasoning(t) => spans.push(Span::styled(flat(t, 240), Style::default().fg(theme::muted()).add_modifier(Modifier::DIM))),
                AgentEvent::ToolCall { name, input } => {
                    spans.push(Span::styled(format!("{} ", tool_icon(name)), cstyle));
                    spans.push(Span::styled(format!("{name}  "), Style::default().fg(theme::secondary())));
                    if let Some(fp) = file_path_of(input) {
                        spans.extend(file_span(&fp));
                    } else {
                        spans.push(Span::styled(flat(input, 160), Style::default().fg(theme::text())));
                    }
                }
                AgentEvent::Command { cmdline, exit } => {
                    spans.push(Span::styled(format!("{} ", tool_icon("Bash")), cstyle));
                    spans.push(Span::styled(flat(cmdline, 200), Style::default().fg(theme::text())));
                    match exit {
                        Some(0) => spans.push(Span::styled("  (exit 0)", Style::default().fg(theme::ok()))),
                        Some(e) => spans.push(Span::styled(format!("  (exit {e})"), Style::default().fg(theme::err()))),
                        None => {}
                    }
                }
                AgentEvent::FileChange(p) => {
                    let g = if icons_on() { "\u{f044} " } else { "✎ " };
                    spans.push(Span::styled(g, Style::default().fg(theme::ok())));
                    for (i, path) in p.iter().enumerate() {
                        if i > 0 {
                            spans.push(Span::raw("  "));
                        }
                        spans.extend(file_span(path));
                    }
                }
                AgentEvent::ToolResult(t) => {
                    spans.truncate(0);
                    spans.push(Span::styled(format!("┃   ↳ {}", flat(t, 200)), dim));
                }
                AgentEvent::Done(t) => {
                    let g = if icons_on() { "\u{f00c} " } else { "✓ " };
                    spans.push(Span::styled(format!("{g}{t}"), Style::default().fg(theme::ok()).add_modifier(Modifier::DIM)));
                }
            }
            Line::from(spans)
        }
    }
}

fn finding_line(f: &FindingRow) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("[{}] ", f.sev), Style::default().fg(theme::severity(&f.sev)).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{}:{} ", f.file, f.line), Style::default().fg(theme::secondary())),
        Span::styled("— ", Style::default().fg(theme::faint())),
        Span::styled(flat(&f.issue, 200), Style::default().fg(theme::text())),
    ])
}

fn draw(f: &mut Frame, app: &mut App) {
    let fh: u16 = if app.findings.is_empty() { 0 } else { (app.findings.len() as u16 + 2).min(8) };
    let chunks = Layout::vertical([
        Constraint::Length(1),  // header
        Constraint::Min(3),     // conversation
        Constraint::Length(fh), // findings
        Constraint::Length(1),  // footer
    ])
    .split(f.area());

    // header
    let status_color = match app.status.as_str() {
        "approve" => theme::ok(),
        "request_changes" => theme::warn(),
        _ => theme::muted(),
    };
    let header = Line::from(vec![
        Span::styled(" ♫ duet ", Style::default().fg(theme::on_accent()).bg(theme::periwinkle()).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {} ", app.title), Style::default().fg(theme::text())),
        Span::styled(format!("· {} ", app.status), Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
    ]);
    f.render_widget(Paragraph::new(header), chunks[0]);

    // conversation
    let lines: Vec<Line> = app.rows.iter().map(|r| row_line(r, chunks[1].width)).collect();
    let total = lines.len();
    let vis = chunks[1].height as usize;
    let max_scroll = total.saturating_sub(vis);
    app.scroll = if app.follow { max_scroll } else { app.scroll.min(max_scroll) };
    f.render_widget(Paragraph::new(lines).scroll((app.scroll as u16, 0)), chunks[1]);

    // findings
    if fh > 0 {
        let fl: Vec<Line> = app.findings.iter().map(finding_line).collect();
        let block = Block::default()
            .borders(Borders::TOP)
            .title(format!(" findings ({}) ", app.findings.len()));
        f.render_widget(Paragraph::new(fl).block(block), chunks[2]);
    }

    // footer
    let follow = if app.follow { "on" } else { "off" };
    let footer = Span::styled(
        format!(" q quit · ↑/↓ scroll · g/G top/bottom · f follow:{follow} "),
        Style::default().add_modifier(Modifier::DIM),
    );
    f.render_widget(Paragraph::new(Line::from(footer)), chunks[3]);
}

/// Interactive viewer (alternate screen, raw mode). Returns on `q`/`Esc`.
pub fn run(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = loop {
        if let Err(e) = term.draw(|f| draw(f, app)) {
            break Err(e.into());
        }
        match event::poll(Duration::from_millis(200)) {
            Ok(true) => {
                if let Ok(Event::Key(k)) = event::read() {
                    if k.kind == KeyEventKind::Press {
                        match k.code {
                            KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                            KeyCode::Down | KeyCode::Char('j') => app.scroll_down(1),
                            KeyCode::Up | KeyCode::Char('k') => app.scroll_up(1),
                            KeyCode::PageDown => app.scroll_down(10),
                            KeyCode::PageUp => app.scroll_up(10),
                            KeyCode::Char('g') => {
                                app.follow = false;
                                app.scroll = 0;
                            }
                            KeyCode::Char('G') => app.follow = true,
                            KeyCode::Char('f') => app.follow = !app.follow,
                            _ => {}
                        }
                    }
                }
            }
            Ok(false) => {}
            Err(e) => break Err(e.into()),
        }
    };

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    result
}

/// Render a single frame to plain text via `TestBackend` (for `--snapshot` and
/// tests — no terminal required).
pub fn snapshot(app: &mut App, width: u16, height: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(width, height)).expect("test backend");
    term.draw(|f| draw(f, app)).expect("draw");
    buffer_to_string(term.backend().buffer())
}

fn buffer_to_string(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out = out.trim_end().to_string();
        out.push('\n');
    }
    out
}

/// LIVE interactive viewer: drains engine messages off `rx` while redrawing and
/// handling keys. The run keeps showing after the engine finishes; `q` exits.
pub fn run_live(rx: Receiver<UiMsg>, title: String) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout))?;
    let mut app = App::live(title);

    let result = loop {
        // drain everything currently available
        loop {
            match rx.try_recv() {
                Ok(m) => app.apply(m),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        if let Err(e) = term.draw(|f| draw(f, &mut app)) {
            break Err(e.into());
        }
        match event::poll(Duration::from_millis(80)) {
            Ok(true) => {
                if let Ok(Event::Key(k)) = event::read() {
                    if k.kind == KeyEventKind::Press {
                        match k.code {
                            KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                            KeyCode::Down | KeyCode::Char('j') => app.scroll_down(1),
                            KeyCode::Up | KeyCode::Char('k') => app.scroll_up(1),
                            KeyCode::PageDown => app.scroll_down(10),
                            KeyCode::PageUp => app.scroll_up(10),
                            KeyCode::Char('g') => {
                                app.follow = false;
                                app.scroll = 0;
                            }
                            KeyCode::Char('G') => app.follow = true,
                            KeyCode::Char('f') => app.follow = !app.follow,
                            _ => {}
                        }
                    }
                }
            }
            Ok(false) => {}
            Err(e) => break Err(e.into()),
        }
    };

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    result
}

fn emit_frame(term: &mut Terminal<TestBackend>, app: &mut App, n: usize, label: &str) {
    term.draw(|f| draw(f, app)).ok();
    println!("\n──────── frame {n} · {label} ────────");
    print!("{}", buffer_to_string(term.backend().buffer()));
}

/// HEADLESS live recorder: same channel-fed App as the interactive viewer, but
/// renders real frames to stdout as the run progresses (on phase boundaries,
/// findings, every ~10 events, and at completion). Lets the live TUI be seen and
/// captured without a terminal.
pub fn record(rx: Receiver<UiMsg>, title: String, width: u16, height: u16) {
    let mut app = App::live(title);
    let mut term = Terminal::new(TestBackend::new(width, height)).expect("test backend");
    let (mut frame_no, mut frames, mut since) = (0usize, 0usize, 0usize);
    const CAP: usize = 30;

    while let Ok(msg) = rx.recv() {
        let mut render = false;
        let mut done = false;
        let mut label = String::new();
        match &msg {
            UiMsg::Phase(p) => {
                render = true;
                label = p.clone();
            }
            UiMsg::Findings(_) => {
                render = true;
                label = "findings".into();
            }
            UiMsg::Done(_) => {
                render = true;
                done = true;
                label = "complete".into();
            }
            UiMsg::Event(_, _) => {
                since += 1;
                if since >= 10 {
                    render = true;
                    label = "…".into();
                }
            }
            _ => {}
        }
        app.apply(msg);
        if (render && frames < CAP) || done {
            frames += 1;
            frame_no += 1;
            since = 0;
            emit_frame(&mut term, &mut app, frame_no, &label);
        }
        if done {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> App {
        App {
            title: "demo".into(),
            status: "request_changes".into(),
            rows: vec![
                Row::Phase("Review · codex".into()),
                Row::Event(Model::Codex, AgentEvent::Command { cmdline: "git diff".into(), exit: Some(0) }),
                Row::Event(Model::Codex, AgentEvent::Message("found a problem".into())),
                Row::Phase("Build · claude".into()),
                Row::Event(Model::Claude, AgentEvent::Message("fixing it now".into())),
            ],
            findings: vec![FindingRow {
                sev: "major".into(),
                file: "mathutils.py".into(),
                line: 7,
                issue: "divides by zero on empty input".into(),
            }],
            scroll: 0,
            follow: true,
        }
    }

    #[test]
    fn renders_header_conversation_and_findings() {
        let mut app = sample();
        let frame = snapshot(&mut app, 80, 20);
        assert!(frame.contains("duet"), "header present");
        assert!(frame.contains("claude") && frame.contains("codex"), "both models attributed");
        assert!(frame.contains("git diff"), "a command line is shown");
        assert!(frame.contains("findings (1)"), "findings panel titled");
        assert!(frame.contains("mathutils.py"), "the finding is listed");
        assert!(frame.contains("quit"), "footer keybindings present");
    }

    #[test]
    fn empty_findings_hides_panel() {
        let mut app = sample();
        app.findings.clear();
        let frame = snapshot(&mut app, 80, 20);
        assert!(!frame.contains("findings ("), "no findings panel when there are none");
    }
}
