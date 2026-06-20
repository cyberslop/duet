# duet

Adversarial, cross-model AI development for the command line. One model writes code, a
second model from a different vendor reviews it, and the two iterate over a working tree
until the reviewer's objections are resolved or a bounded round limit is reached.

The premise is narrow and testable. A model that reviews its own output carries the same
blind spots that produced it. A model from a different lineage tends to fail in different
places, so cross-vendor review surfaces defects that single-model self-review does not.
duet pairs Claude Code (builder) with OpenAI Codex (critic) by default, and can route any
role, including a third opinion, to a local model served over an OpenAI-compatible API.

## Built with duet

duet's own Rust source was written this way. Each change was drafted by one model and
critiqued by another through the same loop the tool automates, with the adversarial
review run before the change was accepted. The codebase is the tool's first and largest
case study, and its integration tests parse real CLI output captured during that
development.

## Capabilities

- **Cross-model adversarial loop.** The builder and critic alternate over bounded rounds
  and always finish on a review. Convergence is decided by an objective gate (a test
  command, a citation check, or a harness smoke test), not by a model grading itself.
- **Interactive shell.** Running `duet` with no arguments opens a full-screen terminal
  application with a header, a scrolling conversation, and an input box. Plain text is
  treated as chat; `/` opens a command palette; `/run <task>` starts the full workflow.
- **Three domains.** `code` runs the build-and-review loop. `research` gathers sources
  and verifies each claim against its citation. `security` builds forensics,
  reverse-engineering, and incident-analysis harnesses under explicit operational-safety
  constraints.
- **Three providers.** Claude Code, OpenAI Codex, and any OpenAI-compatible local server
  (LM Studio, Ollama, vLLM, LiteLLM), assignable per role.
- **Hardware-aware model selection.** duet probes the host's CPU, GPU, and memory and
  recommends local models that fit, ranked by capability and cost, falling back to cloud
  models when no local option is viable.
- **Profiles.** Named role-to-model bundles, defined in the repository and overridable
  per user.
- **Deterministic, replayable runs.** Every model invocation streams typed JSON events
  into a color-coded transcript that is recorded to disk and can be replayed.

## Requirements

- Rust (stable, 2021 edition) to build from source.
- Claude Code (`claude`), authenticated.
- OpenAI Codex CLI (`codex`), authenticated.
- A git repository for the `code` and `security` workflows, which diff the working tree to
  obtain the unit under review.
- Optional: a local OpenAI-compatible server for local-model roles, located with
  `DUET_LOCAL_BASE_URL`.

`duet doctor` reports the status of each prerequisite.

## Install

```bash
git clone https://github.com/cyberslop/duet.git
cd duet
cargo build --release
ln -s "$PWD/target/release/duet" ~/.local/bin/duet
```

## Usage

Open the interactive shell from inside a git repository:

```bash
duet
```

Plain text chats with the default model. A message that clearly describes a build task
starts a planning session automatically. The `/` key opens the command palette, and
`/run <task>` starts the full plan, build, review, fix, and verify loop.

The same workflows are available non-interactively:

```bash
duet run "add a median() function with an empty-input guard and tests"
duet review
duet run --tui "<task>"
duet run --domain research "How does the Eiffel Tower's height change with temperature?"
duet run --domain security "Build a harness to triage AVML memory captures"
```

### Shell commands

| Command | Description |
|---|---|
| `<text>` | Chat with the default model |
| `/run <task>` | Full workflow: plan, build, review, fix, verify |
| `/review [text]`, `/plan <text>` | Review only, or plan only |
| `/domain code\|research\|security` | Switch domain |
| `/builder claude\|codex`, `/critic claude\|codex\|local` | Reassign a role |
| `/profile <name>`, `/profiles` | Apply or list profiles |
| `/rounds <N>`, `/swap`, `/noplan` | Tune the loop |
| `/models`, `/doctor`, `/status` | Local-model advice, environment check, current setup |

## Model selection

duet selects models by capability and cost rather than by whatever happens to be loaded.
A profile bundles a builder, a critic, a round count, and a domain.

```bash
duet profiles
duet run --profile code-local-critic "<task>"
duet suggest-models --domain security
```

The advisor probes the host and recommends local models that fit, falling back to cloud
models when none are viable. Built-in profiles are defined in
`crates/duet-cli/src/profiles.toml`. User overrides live in
`~/.config/duet/profiles.toml`, where a matching name takes precedence.

## Architecture

A four-crate Cargo workspace with a strict dependency order. `duet-cli` depends on
`duet-agents`, `duet-core`, and `duet-tui`. `duet-agents` depends on `duet-core`.
`duet-core` depends on nothing else in the workspace.

```
duet-core    Engine and abstractions: the typed event model (Claude stream-json
             and Codex --json normalized to one AgentEvent type), the Agent,
             Critic, Domain, and Reporter traits, the phase-and-round orchestrator,
             prompts, the hardware probe, and the model advisor.
duet-agents  Concrete backends: the Claude Code and Codex CLI drivers, the local
             OpenAI-compatible HTTP backend, and the Critic implementations.
duet-tui     The ratatui interfaces: the interactive shell and the run viewer.
duet-cli     The duet binary (clap), which wires backends to the engine.
```

Adding a provider is a single `Agent` implementation, plus a `Critic` implementation if
it reviews, in `duet-agents`. Adding a workflow is a single `Domain` implementation in
`duet-core`. The engine references only the traits, never a concrete backend.

## Configuration

| Variable | Purpose |
|---|---|
| `CLAUDE_BIN`, `CODEX_BIN` | Override CLI discovery |
| `DUET_LOCAL_BASE_URL` | Local OpenAI-compatible endpoint (default `http://localhost:1234/v1`) |
| `DUET_LOCAL_MODEL`, `LOCAL_API_KEY` | Local model id and API key |
| `DUET_PROFILES` | Path to a profiles TOML that overrides the default location |
| `DUET_PROMPTS` | Directory of prompt-template overrides |
| `DUET_NO_ICONS` | Disable Nerd-Font file icons and use plain glyphs |
| `NO_COLOR` | Disable ANSI color |

## Development

```bash
cargo test
cargo clippy --all-targets
cargo build --release
```

The event layer is tested offline against real captured CLI streams in
`crates/duet-core/tests/fixtures/`. The terminal interfaces render to a ratatui
`TestBackend`, so layout is verified without a live terminal.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Issues and pull requests are welcome.

## License

MIT. Copyright 2026 CYBERSLOP. See [LICENSE](LICENSE).
