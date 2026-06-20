//! The full-screen interactive shell — `duet` as a persistent terminal app (like
//! Claude Code / Codex / Copilot): a header bar, a scrolling conversation, and an
//! input box at the bottom. The UI lives here; the *logic* (what commands mean,
//! how to run the engine) is a `ShellController` supplied by the CLI.

use crate::{row_line, theme, Row};
use anyhow::Result;
use duet_core::report::UiMsg;
use duet_core::style::Theme;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

const SPIN: &[&str] = &["♪", "♫", "♬", "♩"];

/// What a submitted line should do.
pub enum ShellAction {
    /// Print these lines to the scrollback (command output / help / errors).
    Print(Vec<String>),
    /// Start a workflow run; drain this channel until it sends Done. Ends with a
    /// convergence verdict line.
    Run(Receiver<UiMsg>),
    /// A conversational reply from the default model; drain like a run but end
    /// quietly (no convergence verdict).
    Chat(Receiver<UiMsg>),
    /// Quit the shell.
    Quit,
    /// Nothing to do.
    Nothing,
}

/// The CLI supplies this to give the shell its content and behavior.
pub trait ShellController {
    /// Right-hand status of the header (the "ensemble": builder ⇄ critic · domain).
    fn header(&self) -> String;
    /// The input prompt (use narrow chars so the cursor lines up).
    fn prompt(&self) -> String;
    /// Tab-completion candidates (full-line replacements) for the current input.
    fn complete(&self, line: &str) -> Vec<String>;
    /// Lines shown in the scrollback on launch (the banner).
    fn intro(&self) -> Vec<String>;
    /// Handle a submitted line.
    fn on_input(&mut self, line: &str) -> ShellAction;
}

struct Shell {
    rows: Vec<Row>,
    input: Vec<char>,
    cursor: usize,
    scroll: usize,
    follow: bool,
    history: Vec<String>,
    hist: Option<usize>,
    running: Option<Receiver<UiMsg>>,
    chatting: bool,
    menu_idx: usize,
    menu_hidden: bool,
}

impl Shell {
    fn new(intro: Vec<String>) -> Self {
        let mut rows = Vec::new();
        for l in intro {
            rows.push(Row::Note(l));
        }
        Shell {
            rows,
            input: Vec::new(),
            cursor: 0,
            scroll: 0,
            follow: true,
            history: Vec::new(),
            hist: None,
            running: None,
            chatting: false,
            menu_idx: 0,
            menu_hidden: false,
        }
    }

    /// Candidate menu items for the current input (slash-command palette).
    fn menu(&self, ctrl: &dyn ShellController) -> Vec<String> {
        if self.menu_hidden || self.running.is_some() {
            return vec![];
        }
        let line: String = self.input.iter().collect();
        if !line.starts_with('/') {
            return vec![];
        }
        ctrl.complete(&line).into_iter().filter(|c| c.trim_end() != line.trim_end()).collect()
    }

    /// Called after the input text changes — reopen the menu, reset selection.
    fn edited(&mut self) {
        self.menu_hidden = false;
        self.menu_idx = 0;
    }

    fn note(&mut self, s: impl Into<String>) {
        self.rows.push(Row::Note(s.into()));
    }

    /// Fold one engine message into the scrollback.
    fn apply(&mut self, msg: UiMsg) {
        match msg {
            UiMsg::Phase(p) => self.rows.push(Row::Phase(p)),
            UiMsg::Sys(kind, t) => self.rows.push(Row::Sys(kind, t)),
            UiMsg::Say(m, t) => self.rows.push(Row::Note(format!("[{}] {t}", m.label()))),
            UiMsg::Event(m, ev) => self.rows.push(Row::Event(m, ev)),
            UiMsg::Findings(v) => {
                for f in v {
                    self.rows.push(Row::Finding(f));
                }
            }
            UiMsg::Status(_) | UiMsg::Done(_) => {}
        }
    }

    fn submit(&mut self) -> String {
        let s: String = self.input.iter().collect();
        self.input.clear();
        self.cursor = 0;
        s
    }
}

pub fn run_shell(ctrl: &mut dyn ShellController, th: &Theme) -> Result<i32> {
    enable_raw_mode()?;
    let mut out = std::io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;
    let mut sh = Shell::new(ctrl.intro());
    let result = event_loop(&mut term, &mut sh, ctrl, th);
    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    result
}

fn event_loop(
    term: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    sh: &mut Shell,
    ctrl: &mut dyn ShellController,
    th: &Theme,
) -> Result<i32> {
    let start = std::time::Instant::now();
    let mut spin = 0usize;
    loop {
        // drain an active run
        if sh.running.is_some() {
            let mut done = false;
            loop {
                match sh.running.as_ref().unwrap().try_recv() {
                    Ok(UiMsg::Done(code)) => {
                        if !sh.chatting {
                            match code {
                                0 => sh.rows.push(Row::Verdict(true, "🎵 in harmony — converged ♪".into())),
                                2 => sh.rows.push(Row::Verdict(false, "🎶 still tuning — re-run, or /review to continue".into())),
                                _ => {} // errored: the warning above already explains why
                            }
                        }
                        done = true;
                    }
                    Ok(m) => sh.apply(m),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        done = true;
                        break;
                    }
                }
            }
            sh.follow = true;
            if done {
                sh.running = None;
                sh.chatting = false;
            }
        }

        // 3-second musical equalizer pulse above the input box on launch
        let intro = start.elapsed() < Duration::from_millis(3000);
        term.draw(|f| draw(f, sh, ctrl, spin, intro.then_some(spin)))?;
        spin = spin.wrapping_add(1);

        let timeout = if intro {
            55 // smooth animation
        } else if sh.running.is_some() {
            90
        } else {
            250
        };
        if !event::poll(Duration::from_millis(timeout))? {
            continue;
        }
        let Event::Key(k) = event::read()? else { continue };
        if k.kind != KeyEventKind::Press {
            continue;
        }
        if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d')) {
            return Ok(0);
        }
        if sh.running.is_some() {
            match k.code {
                KeyCode::PageUp => scroll_up(sh, 5),
                KeyCode::PageDown => scroll_down(sh, 5),
                KeyCode::Esc => {
                    sh.running = None;
                    sh.chatting = false;
                    sh.note("⏹ stopped — back to the prompt (any background work finishes on its own)");
                }
                _ => {}
            }
            continue;
        }
        let menu_open = !sh.menu(ctrl).is_empty();
        match k.code {
            KeyCode::Esc => {
                if menu_open {
                    sh.menu_hidden = true;
                } else if !sh.input.is_empty() {
                    sh.input.clear();
                    sh.cursor = 0;
                } else {
                    return Ok(0);
                }
            }
            KeyCode::Char(c) => {
                sh.input.insert(sh.cursor, c);
                sh.cursor += 1;
                sh.edited();
            }
            KeyCode::Backspace => {
                if sh.cursor > 0 {
                    sh.input.remove(sh.cursor - 1);
                    sh.cursor -= 1;
                }
                sh.edited();
            }
            KeyCode::Left => sh.cursor = sh.cursor.saturating_sub(1),
            KeyCode::Right => sh.cursor = (sh.cursor + 1).min(sh.input.len()),
            KeyCode::Home => sh.cursor = 0,
            KeyCode::End => sh.cursor = sh.input.len(),
            KeyCode::PageUp => scroll_up(sh, 5),
            KeyCode::PageDown => scroll_down(sh, 5),
            // ↑/↓ navigate the palette when it's open, else input history
            KeyCode::Up if menu_open => sh.menu_idx = sh.menu_idx.saturating_sub(1),
            KeyCode::Down if menu_open => {
                let n = sh.menu(ctrl).len();
                sh.menu_idx = (sh.menu_idx + 1).min(n.saturating_sub(1));
            }
            KeyCode::Up => history(sh, -1),
            KeyCode::Down => history(sh, 1),
            // Tab accepts the highlighted palette item (fills, never submits)
            KeyCode::Tab if menu_open => accept_menu(sh, ctrl),
            KeyCode::Tab => {}
            KeyCode::Enter => {
                if menu_open {
                    accept_menu(sh, ctrl);
                    // if nothing more to complete (no-arg command), submit it
                    if !sh.menu(ctrl).is_empty() {
                        continue;
                    }
                }
                let line = sh.submit();
                if line.trim().is_empty() {
                    continue;
                }
                sh.history.push(line.clone());
                sh.hist = None;
                sh.note(format!("{}{}", ctrl.prompt(), line));
                sh.follow = true;
                match ctrl.on_input(line.trim()) {
                    ShellAction::Print(lines) => {
                        for l in lines {
                            sh.note(l);
                        }
                    }
                    ShellAction::Run(rx) => {
                        sh.running = Some(rx);
                        sh.chatting = false;
                    }
                    ShellAction::Chat(rx) => {
                        sh.running = Some(rx);
                        sh.chatting = true;
                    }
                    ShellAction::Quit => return Ok(0),
                    ShellAction::Nothing => {}
                }
            }
            _ => {}
        }
        let _ = th;
    }
}

fn scroll_up(sh: &mut Shell, n: usize) {
    sh.follow = false;
    sh.scroll = sh.scroll.saturating_sub(n);
}
fn scroll_down(sh: &mut Shell, n: usize) {
    sh.scroll = sh.scroll.saturating_add(n);
}

fn history(sh: &mut Shell, dir: i32) {
    if sh.history.is_empty() {
        return;
    }
    let idx = match (sh.hist, dir) {
        (None, -1) => sh.history.len() - 1,
        (Some(i), -1) => i.saturating_sub(1),
        (Some(i), 1) if i + 1 < sh.history.len() => i + 1,
        (Some(_), 1) => {
            sh.hist = None;
            sh.input.clear();
            sh.cursor = 0;
            return;
        }
        _ => return,
    };
    sh.hist = Some(idx);
    sh.input = sh.history[idx].chars().collect();
    sh.cursor = sh.input.len();
}

/// Accept the highlighted palette item — replace the input with it.
fn accept_menu(sh: &mut Shell, ctrl: &dyn ShellController) {
    let items = sh.menu(ctrl);
    if items.is_empty() {
        return;
    }
    let idx = sh.menu_idx.min(items.len() - 1);
    sh.input = items[idx].chars().collect();
    sh.cursor = sh.input.len();
    sh.menu_idx = 0;
}

/// One row of equalizer bars — a travelling wave colored blue → periwinkle →
/// violet by amplitude (the design-system Equalizer).
fn eq_bars(frame: usize, width: u16) -> Line<'static> {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let mut spans = vec![Span::raw("   ")];
    for x in 0..width.saturating_sub(3) {
        // two superimposed harmonics → a livelier, less uniform pulse
        let v = ((x as f64) * 0.5 + (frame as f64) * 0.45).sin()
            + 0.5 * ((x as f64) * 0.23 - (frame as f64) * 0.31).sin();
        let h = ((((v + 1.5) / 3.0) * 7.0).round() as usize).min(7);
        spans.push(Span::styled(BLOCKS[h].to_string(), Style::default().fg(theme::eq_tier(h))));
    }
    Line::from(spans)
}

/// Color each voice name (claude / codex / local) in a header string by its hue,
/// leaving separators and emoji in the secondary text color.
fn voice_spans(s: &str) -> Vec<Span<'static>> {
    let sec = Style::default().fg(theme::secondary());
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    for token in s.split_inclusive(' ') {
        let word = token.trim_end();
        let voice = match word {
            "claude" => Some(theme::claude()),
            "codex" => Some(theme::codex()),
            "local" => Some(theme::local()),
            _ => None,
        };
        match voice {
            Some(c) => {
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), sec));
                }
                spans.push(Span::styled(word.to_string(), Style::default().fg(c).add_modifier(Modifier::BOLD)));
                buf.push_str(&token[word.len()..]); // trailing space(s), if any
            }
            None => buf.push_str(token),
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, sec));
    }
    spans
}

/// Approximate terminal column width of a char (emoji ≈ 2, else 1).
fn char_cols(c: char) -> usize {
    if (c as u32) >= 0x1F000 {
        2
    } else {
        1
    }
}

/// Rebuild a `Line` from styled cells, merging runs of the same style.
fn cells_to_line(cells: &[(char, Style)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut text = String::new();
    let mut style: Option<Style> = None;
    for (c, st) in cells {
        if style != Some(*st) {
            if let Some(s) = style.take() {
                spans.push(Span::styled(std::mem::take(&mut text), s));
            }
            style = Some(*st);
        }
        text.push(*c);
    }
    if let Some(s) = style {
        spans.push(Span::styled(text, s));
    }
    Line::from(spans)
}

/// Word-wrap a styled line to `width` columns (display-width aware), preserving
/// span styles. Continuation lines start at column 0; long words hard-break.
fn wrap_line(line: &Line, width: u16) -> Vec<Line<'static>> {
    let width = (width as usize).max(1);
    let cells: Vec<(char, Style)> =
        line.spans.iter().flat_map(|s| s.content.chars().map(move |c| (c, s.style))).collect();
    if cells.iter().map(|(c, _)| char_cols(*c)).sum::<usize>() <= width {
        return vec![cells_to_line(&cells)];
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut row: Vec<(char, Style)> = Vec::new();
    let mut cols = 0usize;
    let mut last_space: Option<usize> = None;
    for (c, st) in cells {
        let cw = char_cols(c);
        if cols + cw > width && !row.is_empty() {
            let cut = last_space.filter(|&s| s > 0).unwrap_or(row.len());
            let mut tail = row.split_off(cut);
            out.push(cells_to_line(&row));
            while tail.first().map(|(c, _)| *c == ' ').unwrap_or(false) {
                tail.remove(0);
            }
            row = tail;
            cols = row.iter().map(|(c, _)| char_cols(*c)).sum();
            last_space = row.iter().rposition(|(c, _)| *c == ' ');
        }
        if c == ' ' {
            last_space = Some(row.len());
        }
        row.push((c, st));
        cols += cw;
    }
    if !row.is_empty() {
        out.push(cells_to_line(&row));
    }
    if out.is_empty() {
        out.push(Line::from(String::new()));
    }
    out
}

fn draw(f: &mut Frame, sh: &mut Shell, ctrl: &dyn ShellController, spin: usize, eq: Option<usize>) {
    let area = f.area();
    let constraints: Vec<Constraint> = if eq.is_some() {
        vec![Constraint::Length(1), Constraint::Min(3), Constraint::Length(1), Constraint::Length(3)]
    } else {
        vec![Constraint::Length(1), Constraint::Min(3), Constraint::Length(3)]
    };
    let chunks = Layout::vertical(constraints).split(area);
    let conv = chunks[1];
    let (eq_chunk, input) = if eq.is_some() { (Some(chunks[2]), chunks[3]) } else { (None, chunks[2]) };

    // header — brand badge · the ensemble (voices colored) · right-aligned status
    let mut left = vec![
        Span::styled(" ♫ duet ", Style::default().fg(theme::on_accent()).bg(theme::periwinkle()).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
    ];
    left.extend(voice_spans(&ctrl.header()));
    let (stext, sstyle) = if sh.running.is_some() {
        (format!("{} performing…", SPIN[spin % SPIN.len()]), Style::default().fg(theme::periwinkle()))
    } else {
        ("ready".to_string(), Style::default().fg(theme::muted()))
    };
    let used: usize = left.iter().flat_map(|s| s.content.chars()).map(char_cols).sum();
    let stat_w: usize = stext.chars().map(char_cols).sum();
    let pad = (chunks[0].width as usize).saturating_sub(used + stat_w + 1);
    left.push(Span::raw(" ".repeat(pad)));
    left.push(Span::styled(stext, sstyle));
    f.render_widget(Paragraph::new(Line::from(left)), chunks[0]);

    // conversation — wrapped to the pane width (exact scroll over visual lines)
    let visual: Vec<Line> = sh.rows.iter().flat_map(|r| wrap_line(&row_line(r, conv.width), conv.width)).collect();
    let total = visual.len();
    let vis = conv.height as usize;
    let max = total.saturating_sub(vis);
    sh.scroll = if sh.follow { max } else { sh.scroll.min(max) };
    f.render_widget(Paragraph::new(visual).scroll((sh.scroll as u16, 0)), conv);

    // equalizer (launch intro)
    if let (Some(frame), Some(ec)) = (eq, eq_chunk) {
        f.render_widget(Paragraph::new(eq_bars(frame, ec.width)), ec);
    }

    // input box
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::periwinkle()).add_modifier(Modifier::DIM));
    let prompt = ctrl.prompt();
    let body = if sh.running.is_some() {
        Line::from(Span::styled(
            format!("{}  the ensemble is on stage…  (PgUp/PgDn scroll · Ctrl-C leave)", SPIN[spin % SPIN.len()]),
            Style::default().fg(theme::muted()),
        ))
    } else {
        let text: String = sh.input.iter().collect();
        Line::from(vec![
            Span::styled(prompt.clone(), Style::default().fg(theme::claude())),
            Span::styled(text, Style::default().fg(theme::text())),
        ])
    };
    f.render_widget(Paragraph::new(body).block(block), input);

    // cursor (inside the input box, after the prompt)
    if sh.running.is_none() {
        let px = prompt.chars().count() as u16;
        let max_x = input.x + input.width.saturating_sub(2);
        let cx = (input.x + 1 + px + sh.cursor as u16).min(max_x);
        f.set_cursor_position(Position::new(cx, input.y + 1));
    }

    // slash-command palette — a selectable list floating above the input box
    let items = sh.menu(ctrl);
    if !items.is_empty() && input.y >= 3 {
        let h = (items.len().min(8) as u16 + 2).min(input.y);
        let w = (items.iter().map(|s| s.trim().chars().count()).max().unwrap_or(8).clamp(14, 46) as u16 + 4)
            .min(area.width.saturating_sub(input.x).max(8));
        let rect = Rect { x: input.x, y: input.y - h, width: w, height: h };
        let list_items: Vec<ListItem> = items.iter().map(|s| ListItem::new(format!(" {}", s.trim()))).collect();
        let list = List::new(list_items)
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(theme::periwinkle())).title(" commands "))
            .highlight_style(Style::default().bg(theme::periwinkle()).fg(theme::on_accent()).add_modifier(Modifier::BOLD))
            .highlight_symbol("▸");
        let mut state = ListState::default();
        state.select(Some(sh.menu_idx.min(items.len() - 1)));
        f.render_widget(Clear, rect);
        f.render_stateful_widget(list, rect, &mut state);
    }
}

// Render one frame to text via TestBackend (for tests / snapshots — no terminal).
#[cfg(test)]
fn snapshot(sh: &mut Shell, ctrl: &dyn ShellController, w: u16, h: u16) -> String {
    snapshot_eq(sh, ctrl, w, h, None)
}

#[cfg(test)]
fn snapshot_eq(sh: &mut Shell, ctrl: &dyn ShellController, w: u16, h: u16, eq: Option<usize>) -> String {
    use ratatui::backend::TestBackend;
    let mut term = Terminal::new(TestBackend::new(w, h)).expect("backend");
    term.draw(|f| draw(f, sh, ctrl, 0, eq)).expect("draw");
    let buf = term.backend().buffer();
    let mut s = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(c) = buf.cell((x, y)) {
                s.push_str(c.symbol());
            }
        }
        s = s.trim_end().to_string();
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestCtrl;
    impl ShellController for TestCtrl {
        fn header(&self) -> String {
            "🎤 claude ⇄ 🎧 codex · 🎼 code · 3 rounds".into()
        }
        fn prompt(&self) -> String {
            "♪ code ▸ ".into()
        }
        fn complete(&self, line: &str) -> Vec<String> {
            let cmds = ["run", "review", "domain", "help", "quit"];
            if let Some(rest) = line.strip_prefix('/') {
                if !rest.contains(' ') {
                    return cmds.iter().filter(|c| c.starts_with(rest)).map(|c| format!("/{c} ")).collect();
                }
            }
            vec![]
        }
        fn intro(&self) -> Vec<String> {
            vec!["welcome to the duet".into()]
        }
        fn on_input(&mut self, _l: &str) -> ShellAction {
            ShellAction::Nothing
        }
    }

    #[test]
    fn shell_renders_header_input_and_scrollback() {
        let mut sh = Shell::new(vec!["welcome to the duet".into()]);
        sh.input = "add a median function".chars().collect();
        sh.cursor = sh.input.len();
        let frame = snapshot(&mut sh, &TestCtrl, 70, 12);
        assert!(frame.contains("duet"), "header badge");
        assert!(frame.contains("claude") && frame.contains("codex"), "ensemble");
        assert!(frame.contains("welcome to the duet"), "scrollback intro");
        assert!(frame.contains("add a median function"), "input box text");
    }

    #[test]
    fn shell_full_frame_with_a_run() {
        use duet_core::events::AgentEvent;
        use duet_core::render::Model;
        let mut sh = Shell::new(vec![
            "   ♪ ♫ ♬   d u e t   ♬ ♫ ♪".into(),
            "   a symphony of models — many voices, one score".into(),
        ]);
        for m in [
            UiMsg::Phase("Build — claude".into()),
            UiMsg::Event(Model::Claude, AgentEvent::ToolCall { name: "Edit".into(), input: r#"{"file_path":"src/mathutils.py"}"#.into() }),
            UiMsg::Event(Model::Claude, AgentEvent::Command { cmdline: "pytest -q".into(), exit: Some(0) }),
            UiMsg::Event(Model::Claude, AgentEvent::FileChange(vec!["src/mathutils.py".into(), "README.md".into()])),
            UiMsg::Phase("Review — codex".into()),
            UiMsg::Event(Model::Codex, AgentEvent::Message("looks solid, one edge case".into())),
        ] {
            sh.apply(m);
        }
        sh.input = "/critic local".chars().collect();
        sh.cursor = sh.input.len();
        std::env::set_var("DUET_NO_ICONS", "1"); // assert basenames font-independently
        let frame = snapshot(&mut sh, &TestCtrl, 78, 18);
        std::env::remove_var("DUET_NO_ICONS");
        println!("\n{frame}");
        assert!(frame.contains("mathutils.py"), "file basename shown");
        assert!(frame.contains("README.md"), "filechange basename shown");
        assert!(frame.contains("Build") && frame.contains("Review"), "phases");
    }

    #[test]
    fn long_reply_wraps_not_clipped() {
        use duet_core::events::AgentEvent;
        use duet_core::render::Model;
        let mut sh = Shell::new(vec![]);
        let long = "Yes. Obviously. (Also: this is a coding planning environment, not really something \
                    to plan and build, so this question is just for fun and the answer is a resounding yes.)";
        sh.apply(UiMsg::Event(Model::Claude, AgentEvent::Message(long.into())));
        let frame = snapshot(&mut sh, &TestCtrl, 40, 14);
        println!("\n{frame}");
        // the tail of the sentence must appear (it would be clipped without wrapping)
        assert!(frame.contains("resounding yes"), "end of long reply is visible (wrapped, not clipped)");
        // and no single rendered row exceeds the width
        assert!(frame.lines().all(|l| l.chars().count() <= 40), "no row overflows the pane width");
    }

    #[test]
    fn slash_palette_pops_up() {
        let mut sh = Shell::new(vec!["hi".into()]);
        sh.input = "/".chars().collect();
        sh.cursor = 1;
        let frame = snapshot(&mut sh, &TestCtrl, 52, 14);
        println!("\n{frame}");
        assert!(frame.contains("commands"), "palette title");
        assert!(frame.contains("run") && frame.contains("domain"), "command items listed");
    }

    #[test]
    fn eq_intro_renders_bars_above_input() {
        let mut sh = Shell::new(vec!["welcome".into()]);
        let frame = snapshot_eq(&mut sh, &TestCtrl, 60, 14, Some(7));
        // a row of block characters appears (the equalizer)
        let bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        assert!(frame.chars().any(|c| bars.contains(&c)), "equalizer bars present");
        println!("\n{frame}");
    }
}
