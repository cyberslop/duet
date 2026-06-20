# duet redesign — UI, README, and graphics

a brand-fidelity pass that brings the app, the README, and the repository's graphics into
alignment with the **duet Design System** (the claude.ai/design project, mirrored as the
`duet-design` Agent Skill). duet was already on-brand in structure and copy; this was a
*fidelity + expression* pass, not a teardown. the design system's `ConversationLine`,
`PhaseDivider`, `Badge`, `ModelChip`, `FindingRow`, and `Equalizer` primitives — and the
full-screen `ui_kits/shell` and `ui_kits/viewer` recreations — were the authoritative
target, imported directly from the project.

## 1. brand graphics → committed to the repo

`assets/` now holds every mark plus four generated, token-exact SVGs:

| file | what it is |
|---|---|
| `duet-mark.svg` · `duet-icon.svg` · `duet-logo.svg` | the interlocking-voices mark, app icon, horizontal lockup |
| `duet-hero.svg` | README masthead — mark + wordmark + tagline + an animated equalizer |
| `duet-shell.svg` | a "screenshot" of the live shell: header, color-guttered conversation, a finding, the equalizer, the input box |
| `duet-loop.svg` | the `plan + red-team → Build ⇄ Review → verify → in harmony` flow |
| `duet-social.svg` | a 1280×640 social / OG card |

the four generated SVGs are produced by `assets/generate.py` straight from the design
tokens, so the graphics and the TUI share one palette. equalizer bars animate via SMIL
(which plays in GitHub's `<img>`-embedded SVGs) and degrade to a tasteful static frame.

## 2. README → rebuilt around the brand

the README opens on the hero, carries brand badges, shows the shell mock under "the
interactive shell" and the loop diagram under "how it works", and applies the lowercase
musical voice to headings — while preserving every piece of verified technical content
(capabilities, the command table, architecture, configuration). a new **design** section
points at `docs/brand.md` and the assets. the configuration table documents the new
`COLORTERM` behavior.

## 3. the app's UI → design tokens as the single source of truth

the TUI rendered close to the brand already; this pass makes the tokens authoritative and
turns semantics into color, faithfully to the imported primitives.

- **`duet-core/src/style.rs`** — the streaming-console `Theme` gained a **truecolor**
  variant (exact brand hex) selected when `COLORTERM=truecolor|24bit`, with the existing
  ANSI-256 theme as the fallback and `plain` for non-TTY / `NO_COLOR`.
- **`duet-tui/src/theme.rs`** (new) — the design palette as ratatui colors: voices, brand
  accents, semantics, text, borders, severity, and file-type accents, each exact hex on
  truecolor terminals and the sampled ANSI-256 index otherwise. it replaces every scattered
  `Color::Indexed(...)` literal in the shell and viewer.
- **semantics → color** (`ConversationLine` / `PhaseDivider` / `FindingRow` parity):
  - `Row` gained `Sys`, `Finding`, and `Verdict` variants so engine semantics are no longer
    flattened into undifferentiated dim notes.
  - **phase dividers** now draw `── label ─────…`, a muted rule that **fills the pane width**.
  - **command exit codes** color green (exit 0) / red (nonzero) in both the console and TUI.
  - **verdicts** render green (`🎵 in harmony`) or amber (`🎶 still tuning`); inline
    **findings** color by severity; the viewer badge is now `♫ duet`; the shell header
    voice-colors `claude`/`codex`/`local` and right-aligns its status.
  - the `✎` file-change and `✓` done glyphs now fall back correctly under `DUET_NO_ICONS`.

verified with `cargo fmt`, `cargo clippy --all-targets` (zero warnings), and `cargo test`
(the shell and viewer render to a ratatui `TestBackend`, so layout stays verifiable without
a terminal).

## 4. docs

[`docs/brand.md`](brand.md) is the in-repo brand reference (voice, color, type, the fixed
glyph cast, motion) so future changes stay coherent; this file records the redesign itself.

## regenerating

```bash
python3 assets/generate.py      # rebuild the four generated SVGs from the tokens
cargo test && cargo clippy --all-targets
```
