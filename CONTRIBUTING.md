# Contributing to duet

Thanks for your interest in improving `duet`! This guide covers how to get set up,
the conventions we follow, and where things live.

## Getting set up

```bash
git clone https://github.com/cyberslop/duet.git
cd duet
cargo build --workspace --all-targets --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

These match what CI runs. The `--locked` flag matters: without it a stale `Cargo.lock`
passes locally but fails CI.

You'll also want the CLIs `duet` drives: [Claude Code](https://claude.com/claude-code)
(`claude`) and the [OpenAI Codex CLI](https://github.com/openai/codex) (`codex`), both
logged in. `duet doctor` verifies your environment. A local OpenAI-compatible server
(e.g. [LM Studio](https://lmstudio.ai)) is optional, for local-model work.

The build lands the `duet` binary at `target/debug/duet` (or `target/release/duet` with
`--release`); that is the binary the Dogfooding section below runs.

## Before you open a PR

- **`cargo test --workspace --locked` is green** and **`cargo clippy --workspace
  --all-targets --all-features --locked -- -D warnings` is clean** (zero warnings is the
  bar). These two are the required CI checks (`Test` and `Clippy`); the `Coverage` job is
  informational and will not block a merge.
- **Work on a feature branch off `main` and open a PR.** `main` is protected, so direct
  pushes are rejected. A PR merges once the required checks pass, all conversations are
  resolved, and one maintainer approves.
- **Format your changed files** before committing (e.g. `cargo fmt -p <crate>`). The
  workspace isn't fully rustfmt-clean yet, so avoid a repo-wide `cargo fmt`, which would
  reformat unrelated files and bury your change.
- New behavior comes with a test. The event layer is tested against real captured CLI
  streams in `crates/duet-core/tests/fixtures/`; TUI changes are tested by rendering to
  a `TestBackend` (see `crates/duet-tui/src/shell.rs`), so layout is verifiable without
  a terminal.
- Keep changes focused. Match the surrounding code's style, naming, and comment
  density rather than introducing a new one.

## Project layout

A 4-crate Cargo workspace (see the README's Architecture section):

| Crate | Responsibility |
|---|---|
| `duet-core` | The engine, the `Agent`/`Critic`/`Domain`/`Reporter` traits, the typed event model, prompts, the hardware probe + model advisor. **Depends on nothing else.** |
| `duet-agents` | Concrete backends (Claude/Codex CLI drivers, the local HTTP backend) and the `Critic` impls. |
| `duet-tui` | The ratatui UIs (interactive shell + run viewer). |
| `duet-cli` | The `duet` binary; wires backends to the engine. |

Dependency edges are strict: `cli → {agents, core, tui}` and `agents → core`. The
engine never references a concrete backend; it talks only to traits.

## Common extension points

- **Add a model provider** → one `impl Agent` (and, if it can critique, one `impl
  Critic`) in `duet-agents`. Nothing downstream changes.
- **Add a workflow/domain** → one `impl Domain` in `duet-core::domain` supplying its
  prompts, the unit under review, a review schema, and an **objective** `verify()`
  gate. The engine's plan → build → review ⇄ fix skeleton stays the same.
- **Tune prompts** → edit `crates/duet-core/prompts/` (or override at runtime with
  `DUET_PROMPTS=<dir>`).
- **Add a profile** → `crates/duet-cli/src/profiles.toml` (or a user override in
  `~/.config/duet/profiles.toml`).

## Dogfooding

`duet` reviews code adversarially, including its own. Running `duet review` (or
`duet run` in a worktree) on your change is a great way to catch issues before a human
review.

## Commit & PR conventions

- Write clear, imperative commit subjects ("Add local critic backend", not "added
  stuff").
- Describe *what* changed and *why* in the PR body; link any related issue.
- A PR that touches the engine or backends should note how it was verified (which
  tests, any live run).

## Licensing

By contributing, you agree that your contributions are licensed under the project's
[MIT License](LICENSE).
