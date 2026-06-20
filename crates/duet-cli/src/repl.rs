//! The interactive `duet` session — a full-screen terminal app (like Claude
//! Code) with a musical theme. This module is the *controller*: it owns the
//! session state and decides what each line means; `duet_tui::run_shell` owns
//! the window (header · scrolling conversation · input box). `duet` is a
//! symphony — two voices today, but the ensemble can grow: 🎤 builder, 🎧 critic,
//! and more.

use crate::profile;
use anyhow::Result;
use duet_core::orchestrate::{execute, Config};
use duet_core::render::Model;
use duet_core::report::{ChannelReporter, Sys, UiMsg};
use duet_core::style::Theme;
use duet_tui::{run_shell, ShellAction, ShellController};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

const COMMANDS: &[&str] = &[
    "run", "review", "plan", "chat", "domain", "builder", "critic", "profile", "profiles", "rounds",
    "swap", "noplan", "repo", "models", "doctor", "status", "help", "quit",
];

struct Session {
    repo: PathBuf,
    domain: String,
    builder: Model,
    critic: Model,
    rounds: usize,
    swap: bool,
    no_plan: bool,
    test_cmd: Option<String>,
    local_endpoint: Option<String>,
    local_model: Option<String>,
}

struct Conductor {
    s: Session,
    profiles: Vec<String>,
    /// True while an engine run thread is live, so a second `/run` (e.g. after Esc
    /// stops following) can't race the first on the same git working tree.
    busy: Arc<AtomicBool>,
}

pub fn run_session(th: &Theme) -> Result<i32> {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        println!("duet's interactive shell needs a terminal.");
        println!("for scripted use: duet run \"<task>\"   ·   duet review   ·   duet --help");
        return Ok(0);
    }
    let mut c = Conductor {
        s: Session {
            repo: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            domain: "code".into(),
            builder: Model::Claude,
            critic: Model::Codex,
            rounds: 3,
            swap: false,
            no_plan: false,
            test_cmd: None,
            local_endpoint: None,
            local_model: None,
        },
        profiles: profile::load_all().iter().map(|p| p.name.clone()).collect(),
        busy: Arc::new(AtomicBool::new(false)),
    };
    run_shell(&mut c, th)
}

impl ShellController for Conductor {
    fn header(&self) -> String {
        format!(
            "🎤 {} + 🎧 {}  ·  🎼 {}  ·  {} rounds{}",
            self.s.builder.label(),
            self.s.critic.label(),
            self.s.domain,
            self.s.rounds,
            if self.s.swap { " · swap" } else { "" },
        )
    }

    fn prompt(&self) -> String {
        format!("♪ {} ▸ ", self.s.domain)
    }

    fn intro(&self) -> Vec<String> {
        vec![
            String::new(),
            "   ♪ ♫ ♬   d u e t   ♬ ♫ ♪".into(),
            "   a symphony of models — many voices, one score".into(),
            String::new(),
            "   the 🎤 builds · the 🎧 critiques · they iterate until they're in harmony".into(),
            "   just type to chat  ·  '/' for commands  ·  /run <task> for the full workflow  ·  /quit".into(),
            String::new(),
        ]
    }

    fn complete(&self, line: &str) -> Vec<String> {
        let Some(rest) = line.strip_prefix('/') else {
            return vec![];
        };
        match rest.split_once(char::is_whitespace) {
            None => COMMANDS.iter().filter(|c| c.starts_with(rest)).map(|c| format!("/{c} ")).collect(),
            Some((cmd, argpart)) => {
                let arg = argpart.trim_start();
                self.arg_options(cmd)
                    .into_iter()
                    .filter(|o| o.starts_with(arg))
                    .map(|o| format!("/{cmd} {o} "))
                    .collect()
            }
        }
    }

    fn on_input(&mut self, line: &str) -> ShellAction {
        if let Some(rest) = line.strip_prefix('/') {
            let mut it = rest.splitn(2, char::is_whitespace);
            let cmd = it.next().unwrap_or("");
            let arg = it.next().unwrap_or("").trim();
            return match cmd {
                "quit" | "exit" | "q" => ShellAction::Quit,
                "help" | "h" | "?" => ShellAction::Print(help_lines()),
                "status" | "st" => ShellAction::Print(self.status_lines()),
                "run" => self.perform(arg, false, false),
                "review" => self.perform(arg, true, false),
                "plan" => {
                    if arg.is_empty() {
                        say("usage: /plan <task>")
                    } else {
                        self.perform(arg, false, true)
                    }
                }
                "domain" | "d" => self.set_domain(arg),
                "builder" => self.set_builder(arg),
                "critic" => self.set_critic(arg),
                "profile" | "p" => self.apply_profile(arg),
                "profiles" => ShellAction::Print(self.profile_lines()),
                "rounds" => match arg.parse::<usize>() {
                    Ok(n) if n >= 1 => {
                        self.s.rounds = n;
                        say(&format!("♪ rounds = {n}"))
                    }
                    _ => say("usage: /rounds <N>"),
                },
                "swap" => {
                    self.s.swap = !self.s.swap;
                    say(&format!("♪ fresh-eyes swap {}", onoff(self.s.swap)))
                }
                "noplan" => {
                    self.s.no_plan = !self.s.no_plan;
                    say(&format!("♪ plan phase {}", onoff(!self.s.no_plan)))
                }
                "repo" => self.set_repo(arg),
                "models" | "m" => ShellAction::Print(self.model_lines()),
                "doctor" => ShellAction::Print(self.doctor_lines()),
                "chat" => self.chat(arg),
                _ => say(&format!("unknown command /{cmd} — try /help (Tab completes)")),
            };
        }
        // plain text: a clear build task jumps into a planning session; else chat
        if task_intent(line) {
            self.perform(line, false, true)
        } else {
            self.chat(line)
        }
    }
}

/// A conservative heuristic: does this read as a software build/implement task
/// (→ jump into a planning session) rather than casual chat?
fn task_intent(msg: &str) -> bool {
    let m = msg.trim().to_lowercase();
    let words: Vec<&str> = m.split_whitespace().collect();
    if words.len() < 3 {
        return false; // too short to be a real task
    }
    const PHRASES: &[&str] = &[
        "build me", "build a", "build an", "implement", "create a", "create an", "write a", "write an",
        "write me", "help me build", "i want to build", "i want you to build", "i need a", "let's build",
        "lets build", "set up a", "scaffold", "refactor the", "refactor this", "add a function",
        "add a feature", "add support", "fix the", "fix a", "fix this", "make me a", "design a",
        "generate a", "port the", "migrate the", "can you build", "can you implement", "can you write",
        "can you add", "can you create",
    ];
    if PHRASES.iter().any(|p| m.contains(p)) {
        return true;
    }
    // Imperative opener with a build-oriented verb (not a question). Deliberately
    // excludes "make" and "fix", whose casual uses ("make it stop", "fix me a
    // coffee") would otherwise hijack chat; those route only via an explicit phrase
    // above ("fix the bug"). Use /chat to force chat on anything that still routes.
    const VERBS: &[&str] = &[
        "build", "create", "implement", "add", "write", "refactor", "scaffold", "design", "generate",
        "port", "migrate", "optimize", "rewrite",
    ];
    !m.ends_with('?') && VERBS.contains(&words[0])
}

impl Conductor {
    fn arg_options(&self, cmd: &str) -> Vec<String> {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        match cmd {
            "domain" | "d" => v(&["code", "research", "security"]),
            "builder" => v(&["claude", "codex"]),
            "critic" => v(&["claude", "codex", "local"]),
            "profile" | "p" => self.profiles.clone(),
            _ => vec![],
        }
    }

    fn perform(&self, task: &str, review_only: bool, plan_only: bool) -> ShellAction {
        if self.busy.load(Ordering::SeqCst) {
            return say("a run is still finishing in the background — give it a moment");
        }
        if self.s.builder == self.s.critic {
            return say("🎤 and 🎧 are the same voice — set a different /critic");
        }
        if task.trim().is_empty() && !review_only {
            return say("give me a task to perform — or /review to critique current changes");
        }
        let cfg = self.build_config(task.to_string(), review_only, plan_only);
        ShellAction::Run(spawn_engine(cfg, self.busy.clone()))
    }

    /// Plain chat with the default (builder) model — read-only, repo-aware.
    fn chat(&self, msg: &str) -> ShellAction {
        if msg.trim().is_empty() {
            return ShellAction::Nothing;
        }
        ShellAction::Chat(spawn_chat(self.s.builder, msg.to_string(), self.s.repo.clone()))
    }

    fn build_config(&self, task: String, review_only: bool, plan_only: bool) -> Config {
        let do_plan = if review_only {
            false
        } else if plan_only {
            true
        } else {
            !self.s.no_plan
        };
        let task = if task.trim().is_empty() && review_only {
            "Review and harden the current uncommitted changes.".into()
        } else {
            task
        };
        Config {
            repo: self.s.repo.clone(),
            task,
            builder: self.s.builder,
            critic: self.s.critic,
            rounds: self.s.rounds,
            do_plan,
            plan_only,
            review_only,
            swap: self.s.swap,
            test_cmd: self.s.test_cmd.clone(),
            claude_model: None,
            codex_model: None,
            codex_build_sandbox: "workspace-write".into(),
            branch: None,
            base_ref: None,
            local_endpoint: self.s.local_endpoint.clone(),
            local_model: self.s.local_model.clone(),
            domain: self.s.domain.clone(),
        }
    }

    fn set_domain(&mut self, arg: &str) -> ShellAction {
        match arg {
            "code" | "research" | "security" => {
                self.s.domain = arg.into();
                say(&format!("🎼 domain = {arg}"))
            }
            _ => say("domain must be: code | research | security"),
        }
    }
    fn set_builder(&mut self, arg: &str) -> ShellAction {
        match Model::parse(arg) {
            Some(Model::Local) => say("a local chat model can't build (no file tools) — use claude|codex"),
            Some(m) => {
                self.s.builder = m;
                say(&format!("🎤 builder = {}", m.label()))
            }
            None => say("builder must be: claude | codex"),
        }
    }
    fn set_critic(&mut self, arg: &str) -> ShellAction {
        match Model::parse(arg) {
            Some(m) => {
                self.s.critic = m;
                say(&format!("🎧 critic = {}", m.label()))
            }
            None => say("critic must be: claude | codex | local"),
        }
    }
    fn apply_profile(&mut self, name: &str) -> ShellAction {
        if name.is_empty() {
            return say("usage: /profile <name>  (Tab to list)");
        }
        match profile::find(name) {
            Ok(p) => {
                if let Some(m) = Model::parse(&p.builder) {
                    self.s.builder = m;
                }
                if let Some(m) = Model::parse(&p.critic) {
                    self.s.critic = m;
                }
                self.s.domain = p.domain;
                self.s.rounds = p.rounds;
                self.s.swap = p.swap;
                self.s.no_plan = p.no_plan;
                self.s.test_cmd = p.test_cmd;
                self.s.local_model = p.local_model;
                self.s.local_endpoint = p.local_endpoint;
                ShellAction::Print(vec![format!("🎼 profile '{name}' takes the stand"), self.ensemble_line()])
            }
            Err(e) => say(&e.to_string()),
        }
    }
    fn set_repo(&mut self, arg: &str) -> ShellAction {
        if arg.is_empty() {
            return say("usage: /repo <path>");
        }
        let p = PathBuf::from(arg);
        let p = p.canonicalize().unwrap_or(p);
        if !p.is_dir() {
            return say(&format!("not a directory: {}", p.display()));
        }
        self.s.repo = p;
        say(&format!("repo = {}", self.s.repo.display()))
    }

    fn ensemble_line(&self) -> String {
        format!(
            "   🎤 {} (builder)  ⇄  🎧 {} (critic)   ·   🎼 {}   ·   {} rounds{}",
            self.s.builder.label(),
            self.s.critic.label(),
            self.s.domain,
            self.s.rounds,
            if self.s.swap { " · swap" } else { "" },
        )
    }

    fn status_lines(&self) -> Vec<String> {
        vec![
            "🎼 the ensemble".into(),
            self.ensemble_line(),
            format!("   repo   {}", self.s.repo.display()),
            format!("   plan {}  ·  swap {}", onoff(!self.s.no_plan), onoff(self.s.swap)),
        ]
    }

    fn profile_lines(&self) -> Vec<String> {
        let mut out = vec!["🎼 ensembles (profiles)".to_string()];
        for p in profile::load_all() {
            let swap = if p.swap { " · swap" } else { "" };
            let dom = if p.domain == "code" { String::new() } else { format!(" · {}", p.domain) };
            out.push(format!("  {:<20} {} + {}  ({} rounds{swap}{dom})", p.name, p.builder, p.critic, p.rounds));
            if let Some(d) = p.description {
                out.push(format!("     {d}"));
            }
        }
        out
    }

    fn model_lines(&self) -> Vec<String> {
        use duet_core::advisor::{advise, tasks_for_domain};
        use duet_core::hardware::detect;
        let hw = detect();
        let mut out = vec![format!("🎼 local voices for '{}' on {}", self.s.domain, hw.summary())];
        for task in tasks_for_domain(&self.s.domain) {
            let a = advise(&hw, task);
            match a.capable {
                Some(r) => out.push(format!("  {:?}: {} {} (~{:.0} GB)  · pull {}", task, r.name, r.quant.label(), r.footprint_gb, r.pull_id)),
                None => out.push(format!("  {:?}: no viable local model — route to cloud", task)),
            }
        }
        out
    }

    fn doctor_lines(&self) -> Vec<String> {
        let mut out = vec!["🎼 instruments".to_string()];
        out.push(format!("  claude  {}", duet_agents::resolve_claude().map(|p| p.display().to_string()).unwrap_or_else(|e| format!("MISSING ({e})"))));
        out.push(format!("  codex   {}", duet_agents::resolve_codex().map(|p| p.display().to_string()).unwrap_or_else(|e| format!("MISSING ({e})"))));
        let ep = duet_agents::default_endpoint();
        match duet_agents::LocalBackend::list_models(&ep, 3) {
            Ok(ms) if !ms.is_empty() => out.push(format!("  local   {ep} → {}", ms.join(", "))),
            Ok(_) => out.push(format!("  local   {ep} reachable, no model loaded")),
            Err(_) => out.push(format!("  local   {ep} not reachable (start LM Studio)")),
        }
        out
    }
}

/// Run the engine on a background thread, returning the channel the shell drains.
/// `busy` is held true for the thread's whole lifetime so a second run can't start
/// while this one may still be touching the git working tree.
fn spawn_engine(cfg: Config, busy: Arc<AtomicBool>) -> Receiver<UiMsg> {
    let (tx, rx) = std::sync::mpsc::channel();
    busy.store(true, Ordering::SeqCst);
    std::thread::spawn(move || {
        let rep = ChannelReporter { tx: tx.clone() };
        let builder = duet_agents::agent_for(cfg.builder);
        // 0 = converged · 2 = ran but didn't converge · 3 = errored before/while running
        let code = match duet_agents::build_critic(cfg.critic, cfg.local_endpoint.as_deref(), cfg.local_model.as_deref(), &rep) {
            Ok(critic) => match execute(&cfg, builder, critic, &rep) {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(UiMsg::Sys(Sys::Warn, format!("run failed: {e}")));
                    3
                }
            },
            Err(e) => {
                let _ = tx.send(UiMsg::Sys(Sys::Warn, format!("{e}")));
                3
            }
        };
        busy.store(false, Ordering::SeqCst);
        let _ = tx.send(UiMsg::Done(code));
    });
    rx
}

/// Stream a one-shot conversational reply from the default model.
fn spawn_chat(model: Model, msg: String, repo: PathBuf) -> Receiver<UiMsg> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rep = ChannelReporter { tx: tx.clone() };
        if let Err(e) = duet_agents::chat(model, &msg, &repo, &rep) {
            let _ = tx.send(UiMsg::Sys(Sys::Warn, format!("{e}")));
        }
        let _ = tx.send(UiMsg::Done(0));
    });
    rx
}

fn help_lines() -> Vec<String> {
    vec![
        "🎵 commands  (type '/' for the palette · Tab/Enter to pick · ↑/↓ to move)".into(),
        "  <text>            chat with the default model (read-only, repo-aware)".into(),
        "  /run <task>       full workflow: plan → build → review⇄fix → verify".into(),
        "  /review [text]    critique & fix the current changes      /plan <text>".into(),
        "  /domain code|research|security    /builder claude|codex    /critic claude|codex|local".into(),
        "  /profile <name>   apply an ensemble        /profiles   list them".into(),
        "  /rounds <N>   /swap   /noplan   /repo <path>".into(),
        "  /models   /doctor   /status   /help   /quit".into(),
        "  PgUp/PgDn scroll · ↑/↓ history · Ctrl-C leave".into(),
    ]
}

fn say(msg: &str) -> ShellAction {
    ShellAction::Print(vec![msg.to_string()])
}
fn onoff(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

#[cfg(test)]
mod tests {
    use super::task_intent;

    #[test]
    fn routes_tasks_not_chat() {
        // tasks → planning session
        for t in ["build me a CLI tool", "implement a binary search", "fix the login bug",
                  "add a median function to stats", "can you write a parser for json"] {
            assert!(task_intent(t), "should be a task: {t}");
        }
        // chat → stays chat (incl. casual lines that open with make/fix)
        for c in ["is nikki the prettiest girl in the world?", "duet", "hello there friend",
                  "what does this project do?", "thanks, that helped a lot", "how are you today",
                  "make it stop doing that", "fix me a coffee please"] {
            assert!(!task_intent(c), "should be chat: {c}");
        }
    }
}
