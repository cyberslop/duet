//! CLI-agent backends: Claude Code and OpenAI Codex driven as subprocesses.

use anyhow::Result;
use duet_core::agent::{run_stream, Agent, Bins, Ctx, Role};
use duet_core::events::{parse_claude_line, parse_codex_line, AgentEvent};
use duet_core::render::Model;
use duet_core::report::Reporter;
use std::path::Path;
use std::process::Command;

/// A lightweight, tool-free conversational turn with a CLI model — the shell's
/// default "just chat" mode. No tools/skills (so a casual message can't spin up
/// an agentic session); streams the reply to `rep`.
pub fn chat(model: Model, message: &str, repo: &Path, rep: &dyn Reporter) -> Result<()> {
    let bins = Bins::resolve()?;
    let dir = repo.join(".duet");
    let _ = std::fs::create_dir_all(&dir);
    let log = dir.join("chat.log");
    let raw = dir.join("chat-stream.jsonl");
    let schema = dir.join("chat.schema.json"); // unused by ReviewText
    let ctx = Ctx {
        bins: &bins,
        repo,
        claude_model: None,
        codex_model: None,
        codex_build_sandbox: "read-only",
        schema: &schema,
        log: &log,
        critic_tools: "",
    };
    let agent = agent_for(model);
    run_stream(&*agent, Role::Chat, message, None, &raw, rep, &ctx)
}

pub struct ClaudeAgent;
pub struct CodexAgent;

impl Agent for ClaudeAgent {
    fn model(&self) -> Model {
        Model::Claude
    }
    fn parse_line(&self, line: &str) -> Vec<AgentEvent> {
        parse_claude_line(line)
    }
    fn command(&self, role: Role, prompt: &str, _out: Option<&Path>, ctx: &Ctx) -> Command {
        let mut c = Command::new(&ctx.bins.claude);
        c.current_dir(ctx.repo).arg("-p").arg(prompt);
        if let Some(m) = ctx.claude_model {
            c.arg("--model").arg(m);
        }
        match role {
            Role::Build => {
                c.arg("--permission-mode").arg("bypassPermissions");
            }
            Role::ReviewJson => {
                let mut tools = String::from("Read Grep Glob Bash(git diff:*) Bash(git log:*) Bash(git show:*) Bash(cat:*) Bash(sed:*)");
                if !ctx.critic_tools.is_empty() {
                    tools.push(' ');
                    tools.push_str(ctx.critic_tools);
                }
                c.args(["--permission-mode", "plan", "--allowedTools"])
                    .arg(tools)
                    .args(["--disallowedTools", "Edit Write"]);
            }
            Role::ReviewText => {
                c.args([
                    "--permission-mode",
                    "plan",
                    "--allowedTools",
                    "Read Grep Glob Bash(git diff:*) Bash(cat:*)",
                    "--disallowedTools",
                    "Edit Write",
                ]);
            }
            Role::Chat => {
                // Pure conversation: deny every tool (incl. Skill/Task) so a casual
                // message never spins up an agentic, skill-running session.
                c.args([
                    "--disallowedTools",
                    "Bash Edit Write Read Grep Glob Task Skill AskUserQuestion WebSearch WebFetch NotebookEdit TodoWrite",
                    "--append-system-prompt",
                    CHAT_SYSTEM,
                ]);
            }
        }
        c.args(["--output-format", "stream-json", "--verbose"]);
        c
    }
}

/// System prompt that makes chat a concise, duet-aware companion.
const CHAT_SYSTEM: &str = "You are the conversational companion inside 'duet', a coding tool that pairs \
two AI models to adversarially plan, build, and review software. You have no tools — just talk. Keep \
replies brief and natural. If the user clearly wants to build, implement, design, or fix something, don't \
attempt it yourself: tell them to start it for real with `/run <task>` (or `/plan <task>` to plan first).";

impl Agent for CodexAgent {
    fn model(&self) -> Model {
        Model::Codex
    }
    fn parse_line(&self, line: &str) -> Vec<AgentEvent> {
        parse_codex_line(line)
    }
    fn command(&self, role: Role, prompt: &str, out: Option<&Path>, ctx: &Ctx) -> Command {
        let mut c = Command::new(&ctx.bins.codex);
        c.arg("exec")
            .arg("--json")
            .arg("-C")
            .arg(ctx.repo)
            .arg("--skip-git-repo-check")
            .arg("--color")
            .arg("never");
        if let Some(m) = ctx.codex_model {
            c.arg("-m").arg(m);
        }
        match role {
            Role::Build => {
                c.arg("-s").arg(ctx.codex_build_sandbox);
            }
            Role::ReviewJson => {
                c.arg("-s").arg("read-only").arg("--output-schema").arg(ctx.schema);
            }
            Role::ReviewText | Role::Chat => {
                c.arg("-s").arg("read-only");
            }
        }
        if let Some(o) = out {
            c.arg("-o").arg(o);
        }
        c.arg(prompt);
        c
    }
}

pub fn agent_for(model: Model) -> Box<dyn Agent> {
    match model {
        Model::Claude => Box::new(ClaudeAgent),
        Model::Codex => Box::new(CodexAgent),
        // local is a critic-only HTTP backend, never a CLI agent; the CLI guards
        // against routing it here (use `duet local-review`).
        Model::Local => unreachable!("local backend is not a CLI agent"),
    }
}
