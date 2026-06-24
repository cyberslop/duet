//! The `Agent` abstraction: how to build a non-interactive command for a model
//! in a given role, how to parse its stream, and a shared streaming runner.
//!
//! Adding a new model = implementing this trait once. Everything downstream
//! (orchestration, rendering, the future TUI) is provider-agnostic.

use crate::events::AgentEvent;
use crate::render::Model;
use crate::report::Reporter;
use anyhow::{anyhow, Result};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Resolved executables. `claude` is frequently a shell *alias*, so we fall back
/// to its known install location; both can be overridden via env.
pub struct Bins {
    pub claude: PathBuf,
    pub codex: PathBuf,
}

impl Bins {
    pub fn resolve() -> Result<Self> {
        Ok(Bins { claude: resolve_claude()?, codex: resolve_codex()? })
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|p| p.join(name))
        .find(|p| p.is_file())
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn resolve_claude() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("CLAUDE_BIN").map(PathBuf::from).filter(|p| p.is_file()) {
        return Ok(p);
    }
    if let Some(p) = which("claude") {
        return Ok(p);
    }
    if let Some(p) = home().map(|h| h.join(".claude/local/claude")).filter(|p| p.is_file()) {
        return Ok(p);
    }
    Err(anyhow!(
        "claude (Claude Code) not found on PATH or ~/.claude/local — set CLAUDE_BIN"
    ))
}

pub fn resolve_codex() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("CODEX_BIN").map(PathBuf::from).filter(|p| p.is_file()) {
        return Ok(p);
    }
    if let Some(p) = which("codex") {
        return Ok(p);
    }
    let candidates = [
        home().map(|h| h.join(".local/bin/codex")),
        Some(PathBuf::from("/opt/homebrew/bin/codex")),
    ];
    for c in candidates.into_iter().flatten() {
        if c.is_file() {
            return Ok(c);
        }
    }
    Err(anyhow!(
        "codex not found — install with 'brew install codex' then 'codex login', or set CODEX_BIN"
    ))
}

#[derive(Clone, Copy)]
pub enum Role {
    /// Build: edits files autonomously, may run commands and commit.
    Build,
    /// Critique producing structured JSON (findings).
    ReviewJson,
    /// Critique producing freeform markdown (plan red-team).
    ReviewText,
    /// Long-horizon direction (conductor mode): reads the repo and emits a plan +
    /// the next scoped objective as a structured JSON message. Reads, never edits.
    Strategize,
    /// Plain conversation: no tools, no skills — just a reply.
    Chat,
}

/// Shared, borrowed context for a single model invocation.
pub struct Ctx<'a> {
    pub bins: &'a Bins,
    pub repo: &'a Path,
    pub claude_model: Option<&'a str>,
    pub codex_model: Option<&'a str>,
    pub codex_build_sandbox: &'a str,
    pub schema: &'a Path,
    pub log: &'a Path,
    /// Extra allowed tools a CLI critic needs for this domain (e.g. web access
    /// for research). Empty for code → code parity preserved.
    pub critic_tools: &'a str,
}

pub trait Agent: Send {
    fn model(&self) -> Model;
    fn parse_line(&self, line: &str) -> Vec<AgentEvent>;
    /// Build the non-interactive command. `out` is the `-o`/result file when set.
    fn command(&self, role: Role, prompt: &str, out: Option<&Path>, ctx: &Ctx) -> Command;
}

/// A critic backend: produces a review of the unit to `req.out`, streaming events
/// to `rep`. Implemented in `duet-agents` by a CLI-agent wrapper (reads files via
/// tools) and the local HTTP backend (the unit is inlined into its prompt).
pub trait Critic: Send {
    fn model(&self) -> Model;
    /// CLI critics can also build — eligible for a fresh-eyes role swap.
    fn can_build(&self) -> bool;
    fn review(&self, req: &ReviewReq, ctx: &Ctx, rep: &dyn Reporter) -> Result<()>;
    /// Swap roles: return (new builder, new critic). A CLI critic surrenders its
    /// agent to build and wraps the old builder; guarded by `can_build`.
    fn swapped(self: Box<Self>, builder: Box<dyn Agent>) -> (Box<dyn Agent>, Box<dyn Critic>);
}

/// Everything a critic needs for one review. The engine fills both the CLI field
/// (`cli_prompt`, for tool-using critics) and the inlined-unit fields
/// (`local_system`/`local_user`, for tool-less critics).
pub struct ReviewReq<'a> {
    pub role: Role,
    pub cli_prompt: &'a str,
    pub out: &'a Path,
    pub raw: &'a Path,
    pub schema: &'a str,
    pub local_system: &'a str,
    pub local_user: &'a str,
    pub strip: bool,
}


/// Spawn the agent, stream + render its events live, and tee the raw JSONL to
/// `raw` for replay. stdin is `/dev/null` — without this `codex exec` blocks
/// forever waiting on a piped, non-tty stdin for EOF.
pub fn run_stream(
    agent: &dyn Agent,
    role: Role,
    prompt: &str,
    out: Option<&Path>,
    raw: &Path,
    reporter: &dyn Reporter,
    ctx: &Ctx,
) -> Result<()> {
    let mut cmd = agent.command(role, prompt, out, ctx);
    let log = std::fs::OpenOptions::new().create(true).append(true).open(ctx.log)?;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|e| anyhow!("failed to spawn {}: {e}", agent.model().label()))?;

    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout from child"))?;
    let mut rawf = File::create(raw)?;
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        writeln!(rawf, "{line}")?;
        for ev in agent.parse_line(&line) {
            reporter.event(agent.model(), &ev);
        }
    }
    child.wait()?;
    Ok(())
}
