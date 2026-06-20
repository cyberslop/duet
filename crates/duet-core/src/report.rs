//! `Reporter` decouples the orchestrator from *where* progress goes. The engine
//! only calls `reporter.phase/event/say/...`; a `ConsoleReporter` prints the
//! streaming view, while a `ChannelReporter` forwards everything as [`UiMsg`]s to
//! the live TUI. This is what makes the same engine drive both the headless and
//! the TUI front-ends.

use crate::events::AgentEvent;
use crate::render::{render_line, Model};
use crate::style::Theme;
use std::sync::mpsc::Sender;

/// A single critique finding, domain-neutral. Lives in core (not the TUI) so the
/// engine, console, and TUI all share one type.
#[derive(Clone)]
pub struct FindingRow {
    pub sev: String,
    pub file: String,
    pub line: i64,
    pub issue: String,
}

#[derive(Clone, Copy)]
pub enum Sys {
    Note,
    Ok,
    Warn,
}

pub trait Reporter: Send {
    fn phase(&self, label: &str);
    fn sys(&self, kind: Sys, text: &str);
    fn say(&self, model: Model, text: &str);
    fn event(&self, model: Model, ev: &AgentEvent);
    fn findings(&self, items: &[FindingRow]);
    fn status(&self, verdict: &str);
}

/// Prints the streaming conversation view to stdout (the default front-end).
pub struct ConsoleReporter {
    pub theme: Theme,
}

impl Reporter for ConsoleReporter {
    fn phase(&self, label: &str) {
        let th = &self.theme;
        println!("{}{}{}", th.dim, "─".repeat(60), th.rst);
        println!("{}▸ {label}{}", th.bold, th.rst);
        println!("{}{}{}", th.dim, "─".repeat(60), th.rst);
    }
    fn sys(&self, kind: Sys, text: &str) {
        let th = &self.theme;
        match kind {
            Sys::Note => println!("{}  {text}{}", th.dim, th.rst),
            Sys::Ok => println!("{}✓ {text}{}", th.ok, th.rst),
            Sys::Warn => println!("{}! {text}{}", th.warn, th.rst),
        }
    }
    fn say(&self, model: Model, text: &str) {
        let th = &self.theme;
        println!("{}[{}]{} {text}", model.color(th), model.label(), th.rst);
    }
    fn event(&self, model: Model, ev: &AgentEvent) {
        if let Some(s) = render_line(&self.theme, model, ev) {
            println!("{s}");
        }
    }
    fn findings(&self, items: &[FindingRow]) {
        for f in items {
            println!("  [{}] {}:{} — {}", f.sev, f.file, f.line, f.issue);
        }
    }
    fn status(&self, _verdict: &str) {}
}

/// One message from the engine to the live TUI.
pub enum UiMsg {
    Phase(String),
    Sys(Sys, String),
    Say(Model, String),
    Event(Model, AgentEvent),
    Findings(Vec<FindingRow>),
    Status(String),
    Done(i32),
}

/// Forwards engine progress to the TUI over a channel.
pub struct ChannelReporter {
    pub tx: Sender<UiMsg>,
}

impl Reporter for ChannelReporter {
    fn phase(&self, label: &str) {
        let _ = self.tx.send(UiMsg::Phase(label.into()));
    }
    fn sys(&self, kind: Sys, text: &str) {
        let _ = self.tx.send(UiMsg::Sys(kind, text.into()));
    }
    fn say(&self, model: Model, text: &str) {
        let _ = self.tx.send(UiMsg::Say(model, text.into()));
    }
    fn event(&self, model: Model, ev: &AgentEvent) {
        let _ = self.tx.send(UiMsg::Event(model, ev.clone()));
    }
    fn findings(&self, items: &[FindingRow]) {
        let _ = self.tx.send(UiMsg::Findings(items.to_vec()));
    }
    fn status(&self, verdict: &str) {
        let _ = self.tx.send(UiMsg::Status(verdict.into()));
    }
}
