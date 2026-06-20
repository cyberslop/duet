//! Concrete backends for duet: the CLI agents (Claude Code, OpenAI Codex) and
//! the local OpenAI-compatible HTTP backend (LM Studio / LiteLLM / vLLM / Ollama),
//! plus the `Critic` implementations that wrap them.
//!
//! The dependency edge is strictly `cli → agents → core`: the engine in
//! `duet-core` talks only to the `Agent`/`Critic` traits, never to a concrete
//! backend, so adding a provider is a change confined to this crate.

mod cli;
mod critic;
mod local;

pub use cli::{agent_for, chat, ClaudeAgent, CodexAgent};
pub use critic::{build_critic, CliCritic, LocalCritic};
pub use local::{default_endpoint, LocalBackend};

// Re-export the core abstractions callers (CLI / this crate) build against.
pub use duet_core::agent::{
    resolve_claude, resolve_codex, run_stream, Agent, Bins, Critic, Ctx, ReviewReq, Role,
};
