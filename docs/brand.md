# duet — brand & design reference

> _a symphony of models · many voices, one score._

duet is **dark, monospace, terminal-first**, organized around a musical **duet**
metaphor: two model voices (🎤 builder + 🎧 critic) iterating "until they're in harmony."
this file is the in-repo summary of that system so the TUI, README, and graphics stay
coherent. the full design system lives in the **duet Design System** project on
claude.ai/design (mirrored to the `duet-design` Agent Skill); the colors below are the
source of truth, lifted into code in `crates/duet-core/src/style.rs` and
`crates/duet-tui/src/theme.rs`.

## voice

- **lowercase by default**, terminal-style — `duet run <task>`, `ready`, `still tuning`,
  `in harmony`. Title Case only for **phase labels** (`Build`, `Review`, `Plan red-team`)
  and proper nouns (Claude Code, Codex, LM Studio). never ALL-CAPS for emphasis.
- **imperative / impersonal** — describe *what is happening* (`the 🎤 builds · the 🎧
  critiques`), not "I" or "you". chat replies may use "I" sparingly.
- **the musical register, used deliberately** — models are **voices**; a builder+critic
  pairing is an **ensemble** (also the word for a profile); a profile "**takes the
  stand**"; a run is the ensemble "**on stage**". convergence = **"in harmony"**;
  non-convergence = **"still tuning"**.
- **precision over flourish.** status and error lines are direct and actionable
  (`local http://localhost:1234/v1 not reachable (start LM Studio)`). the metaphor
  decorates; it never obscures the instruction.
- use ` — ` for asides and ` · ` as a compact separator (`3 rounds · swap`).

## color

every color is meaning. each model speaks in its own hue, carried by a thin vertical
gutter `┃` down the left of every line.

| token | hex | ANSI-256 | role |
|---|---|---|---|
| violet | `#A884FF` | 141 | builder voice / primary brand accent |
| azure | `#3AA0FF` | 39 | critic voice / secondary brand accent |
| periwinkle | `#8787FF` | 105 | UI accent — `♫ duet` badge, input border, palette selection |
| claude | `#AF87FF` | 141 | builder voice gutter |
| codex | `#00AFFF` | 39 | critic voice gutter |
| local | `#00D7D7` | 44 | local-model voice gutter |
| ok | `#3FB950` | 40 | converged ✓ / pass / exit 0 |
| warn | `#D29922` | 178 | still tuning ! / request_changes |
| err | `#F85149` | 203 | failed / blocker / nonzero exit |
| ground | `#15171F` | — | charcoal-navy background |
| text | `#ECEEF6` · `#AAB0C2` · `#6B7282` · `#454B5C` | — | primary / secondary / muted / faint |

surfaces step up rather than casting shadow: inset `#0F1117` → base `#15171F` → panel
`#1B1E2A` → raised `#232736`. file-type accent dots: `.rs` `#FF8700`, `.py` `#0087FF`,
`.md` `#5FAFFF`, `.json` `#D7AF00`, `.sh` `#87FF87`, `.js`/`.ts` `#FFFF5F`, `.diff`
`#D75F87`.

**truecolor with fallback.** on terminals that advertise `COLORTERM=truecolor|24bit` the
exact hex above is used; otherwise the ANSI-256 index it was sampled from stands in, so the
palette is on-brand everywhere (including macOS Terminal). `NO_COLOR` disables color in the
streaming console view.

## type & layout

- **mono carries everything** — JetBrains Mono (substitute for the terminal font) for
  messages, commands, paths, prompt, labels. the product *is* a terminal.
- a **4px base grid**; the signature row is **gutter → fixed-width voice label → content**.
- app regions are fixed: a 1-line header, a flexible conversation pane, an optional
  findings strip, and a bordered input box pinned to the bottom. the slash palette floats
  above the input.

## iconography — the fixed glyph cast

duet's primary iconography is a small set of Unicode glyphs that render in any monospace
font. reuse them verbatim; never introduce new decorative emoji.

- **music:** `♪ ♫ ♬ ♩` (the duet signature / 4-frame spinner), equalizer blocks
  `▁ ▂ ▃ ▄ ▅ ▆ ▇ █`.
- **conversation:** `┃` gutter · `⚙` tool/command · `✎` file change · `✓` done/pass ·
  `↳` tool result · `▸` palette selection · `── … ──` phase rule · `⇄` role pairing ·
  `⏹` stop.
- **emoji cast (functional, not confetti):** 🎤 builder · 🎧 critic · 🎼 domain/ensemble ·
  🎵 / 🎶 verdicts.

the richer `duet tui` also draws Nerd-Font file/tool icons with per-extension colors;
`DUET_NO_ICONS=1` falls back to the plain glyphs above (`·` for files, `⚙` for tools).

## motion

- **signature:** the launch **equalizer** — a travelling wave of block bars colored blue →
  periwinkle → violet, pulsing for ~3s on shell start.
- a 4-frame **note spinner** (`♪ ♫ ♬ ♩`) marks "performing…", and a block caret blinks in
  the input box. easing is gentle; respect `prefers-reduced-motion`.

## assets

`assets/` holds the marks (`duet-mark`, `duet-icon`, `duet-logo`) and the generated README
graphics (`duet-hero`, `duet-shell`, `duet-loop`, `duet-social`). the generated ones are
produced from these tokens by `python3 assets/generate.py` — edit the generator, not the
SVG output, and keep new graphics on the palette above.
