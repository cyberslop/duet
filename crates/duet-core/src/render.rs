//! Render a normalized [`AgentEvent`] as one attributed, color-guttered line —
//! the live "watch the two agents talk" view. (A ratatui TUI will consume the
//! same [`AgentEvent`] stream; this streaming renderer is the headless form.)

use crate::events::AgentEvent;
use crate::style::Theme;

#[derive(Clone, Copy, PartialEq)]
pub enum Model {
    Claude,
    Codex,
    Local,
}

impl Model {
    pub fn label(self) -> &'static str {
        match self {
            Model::Claude => "claude",
            Model::Codex => "codex",
            Model::Local => "local",
        }
    }

    pub fn color(self, th: &Theme) -> &'static str {
        match self {
            Model::Claude => th.claude,
            Model::Codex => th.codex,
            Model::Local => th.local,
        }
    }

    pub fn parse(s: &str) -> Option<Model> {
        match s {
            "claude" => Some(Model::Claude),
            "codex" => Some(Model::Codex),
            "local" => Some(Model::Local),
            _ => None,
        }
    }
}

/// Collapse all runs of whitespace (incl. newlines/tabs) to single spaces and
/// truncate, so each event renders as exactly one terminal line.
fn flat(s: &str, max: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > max {
        let mut t: String = one.chars().take(max).collect();
        t.push('…');
        t
    } else {
        one
    }
}

/// Format one event for the live view. Returns `None` for events that should
/// produce no visible line (e.g. empty text).
pub fn render_line(th: &Theme, model: Model, ev: &AgentEvent) -> Option<String> {
    let c = model.color(th);
    let (rst, dim, ok, err) = (th.rst, th.dim, th.ok, th.err);
    let lab = format!("{:<6}", model.label());
    let bar = format!("{c}┃{rst}");

    let line = match ev {
        AgentEvent::Message(t) => {
            let body = flat(t, 400);
            if body.is_empty() {
                return None;
            }
            format!("{bar} {c}{lab}{rst} {body}")
        }
        AgentEvent::Reasoning(t) => {
            let body = flat(t, 200);
            format!("{bar} {dim}{lab}{rst} {dim}{body}{rst}")
        }
        AgentEvent::ToolCall { name, input } => {
            let body = flat(input, 160);
            format!("{bar} {c}{lab}{rst} {c}⚙{rst} {name}  {body}")
        }
        AgentEvent::Command { cmdline, exit } => {
            let body = flat(cmdline, 160);
            let ex = match exit {
                Some(0) => format!("  {ok}(exit 0){rst}"),
                Some(e) => format!("  {err}(exit {e}){rst}"),
                None => String::new(),
            };
            format!("{bar} {c}{lab}{rst} {c}⚙{rst} {body}{ex}")
        }
        AgentEvent::FileChange(paths) => {
            let body = paths.join(", ");
            format!("{bar} {c}{lab}{rst} {ok}✎ {body}{rst}")
        }
        AgentEvent::ToolResult(t) => {
            let body = flat(t, 160);
            if body.is_empty() {
                return None;
            }
            format!("{bar} {dim}{lab}   ↳ {body}{rst}")
        }
        AgentEvent::Done(t) => format!("{bar} {dim}{lab}   ✓ {t}{rst}"),
    };
    Some(line)
}

/// Render every line of a captured JSONL stream for a given model (used by
/// `duet replay`). `parse` selects the provider parser.
pub fn render_stream(th: &Theme, model: Model, jsonl: &str) {
    let parse: fn(&str) -> Vec<AgentEvent> = match model {
        Model::Claude => crate::events::parse_claude_line,
        Model::Codex => crate::events::parse_codex_line,
        Model::Local => crate::events::parse_local_line,
    };
    for line in jsonl.lines() {
        for ev in parse(line) {
            if let Some(s) = render_line(th, model, &ev) {
                println!("{s}");
            }
        }
    }
}

/// Infer the model from a saved stream filename like `stream-03-review-codex.jsonl`.
pub fn model_of_filename(name: &str) -> Option<Model> {
    if name.contains("claude") {
        Some(Model::Claude)
    } else if name.contains("codex") {
        Some(Model::Codex)
    } else if name.contains("local") {
        Some(Model::Local)
    } else {
        None
    }
}
