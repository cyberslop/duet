#!/usr/bin/env python3
"""Generate duet's brand graphics from the design tokens.

duet is a dark, terminal-first tool with a musical "duet" metaphor: two model
voices (builder + critic) iterating until they're in harmony. Every color here
is lifted verbatim from the duet design system (see docs/brand.md), so the
README graphics and the TUI speak the same palette.

    python3 assets/generate.py        # writes the *.svg files in this folder

Hand-authored marks (duet-mark / duet-icon / duet-logo) are NOT regenerated.
"""

import math
import os
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))

# ── design tokens ───────────────────────────────────────────────────────────
VIOLET, AZURE, PERIWINKLE = "#A884FF", "#3AA0FF", "#8787FF"
VIOLET_HI, AZURE_HI = "#B89BFF", "#5AB0FF"
BLEND_MID, VIOLET_LO = "#8E92FF", "#9F7BFF"  # the --duet-blend seam + mark gradient stops
V_CLAUDE, V_CODEX, V_LOCAL = "#AF87FF", "#00AFFF", "#00D7D7"
BG, PANEL, RAISED, INSET = "#15171F", "#1B1E2A", "#232736", "#0F1117"
TX, TX2, TX_MUTE, TX_FAINT = "#ECEEF6", "#AAB0C2", "#6B7282", "#454B5C"
OK, WARN, ERR = "#3FB950", "#D29922", "#F85149"
B_SUBTLE, B_STRONG = "#262B3A", "#353B4E"
F_PY, F_RUST, F_MD = "#0087FF", "#FF8700", "#5FAFFF"
MONO = "'JetBrains Mono','SF Mono','Cascadia Code',ui-monospace,Menlo,Consolas,monospace"


def _hex(c):
    return tuple(int(c[i : i + 2], 16) for i in (1, 3, 5))


def lerp(a, b, t):
    ca, cb = _hex(a), _hex(b)
    return "#%02X%02X%02X" % tuple(round(ca[i] + (cb[i] - ca[i]) * t) for i in range(3))


def spectrum(t):
    """blue -> periwinkle -> violet, the equalizer wave."""
    return lerp(AZURE, PERIWINKLE, t * 2) if t < 0.5 else lerp(PERIWINKLE, VIOLET, (t - 0.5) * 2)


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


# ── building blocks ─────────────────────────────────────────────────────────
def mark(x, y, size, uid):
    """The two interlocking voices (from duet-mark.svg), scaled into a box."""
    s = size / 120.0
    return f"""
  <g transform="translate({x},{y}) scale({s:.4f})">
    <defs>
      <linearGradient id="mv{uid}" x1="20" y1="14" x2="62" y2="70" gradientUnits="userSpaceOnUse">
        <stop stop-color="{VIOLET_HI}"/><stop offset="1" stop-color="{VIOLET_LO}"/>
      </linearGradient>
      <linearGradient id="ma{uid}" x1="100" y1="106" x2="58" y2="50" gradientUnits="userSpaceOnUse">
        <stop stop-color="{AZURE}"/><stop offset="1" stop-color="{AZURE_HI}"/>
      </linearGradient>
    </defs>
    <path d="M60 16 A36 36 0 0 0 60 88" stroke="url(#mv{uid})" stroke-width="17" stroke-linecap="round"/>
    <path d="M60 104 A36 36 0 0 0 60 32" stroke="url(#ma{uid})" stroke-width="17" stroke-linecap="round"/>
  </g>"""


def equalizer(x, base, n, bw, gap, peak, animate=True, seed=0.0):
    """A travelling wave of block bars, colored across the spectrum left->right.
    Bars scale vertically from their base (a faint baseline anchors them)."""
    out = [f'  <line x1="{x}" y1="{base+0.5}" x2="{x + n*(bw+gap) - gap}" y2="{base+0.5}" '
           f'stroke="{B_SUBTLE}" stroke-width="1"/>']
    for i in range(n):
        bx = x + i * (bw + gap)
        phase = seed + i * 0.5
        h = (0.45 + 0.55 * (0.5 + 0.5 * math.sin(phase))) * peak
        col = spectrum(i / max(1, n - 1))
        anim = ""
        if animate:
            lo, hi = 0.34, 1.0
            a = round(0.4 + 0.6 * (0.5 + 0.5 * math.sin(phase)), 3)
            b = round(0.4 + 0.6 * (0.5 + 0.5 * math.sin(phase + 2.4)), 3)
            dur = round(1.05 + 0.012 * ((i * 7) % 9), 3)
            anim = (f'<animateTransform attributeName="transform" type="scale" '
                    f'values="1 {a};1 {hi};1 {b};1 {lo};1 {a}" keyTimes="0;0.3;0.55;0.8;1" '
                    f'dur="{dur}s" begin="-{round(i*0.06,3)}s" repeatCount="indefinite" '
                    f'calcMode="spline" keySplines="0.16 1 0.3 1;0.16 1 0.3 1;0.4 0 0.2 1;0.16 1 0.3 1"/>')
        out.append(f'  <g transform="translate({bx:.1f},{base})"><rect x="0" y="{-h:.1f}" '
                   f'width="{bw}" height="{h:.1f}" rx="2" fill="{col}">{anim}</rect></g>')
    return "\n".join(out)


def caret(x, y, h):
    return (f'  <rect x="{x}" y="{y}" width="9" height="{h}" rx="1.5" fill="{V_CLAUDE}">'
            f'<animate attributeName="opacity" values="1;1;0;0" keyTimes="0;0.5;0.5;1" '
            f'dur="1.06s" repeatCount="indefinite"/></rect>')


def svg(w, h, body, defs="", rounded=0):
    bg = (f'<rect width="{w}" height="{h}" rx="{rounded}" fill="{BG}"/>' if rounded
          else f'<rect width="{w}" height="{h}" fill="{BG}"/>')
    return (f'<svg width="{w}" height="{h}" viewBox="0 0 {w} {h}" fill="none" '
            f'xmlns="http://www.w3.org/2000/svg" font-family="{MONO}">\n'
            f'  <defs>{defs}</defs>\n  {bg}\n{body}\n</svg>\n')


def write(name, content):
    ET.fromstring(content)  # validate well-formedness
    with open(os.path.join(HERE, name), "w") as f:
        f.write(content)
    print(f"  wrote {name}  ({len(content)} bytes)")


def text(x, y, s, fill=TX, size=15, weight=400, anchor="start", spacing="0", extra=""):
    return (f'<text x="{x}" y="{y}" fill="{fill}" font-size="{size}" font-weight="{weight}" '
            f'text-anchor="{anchor}" letter-spacing="{spacing}" {extra}>{s}</text>')


def wordmark(x, y, size, uid):
    """'duet' with the violet->azure blend swept across the letters."""
    g = (f'<linearGradient id="wm{uid}" x1="0" y1="0" x2="1" y2="0">'
         f'<stop offset="0" stop-color="{VIOLET}"/><stop offset="0.5" stop-color="{BLEND_MID}"/>'
         f'<stop offset="1" stop-color="{AZURE}"/></linearGradient>')
    t = (f'<text x="{x}" y="{y}" fill="url(#wm{uid})" font-size="{size}" font-weight="800" '
         f'letter-spacing="-2">duet</text>')
    return g, t


# ── 1. hero (README masthead) ───────────────────────────────────────────────
def hero():
    W, H = 1280, 320
    g, wm = wordmark(250, 178, 116, "h")
    defs = (g + f'<linearGradient id="seam" x1="0" y1="0" x2="1" y2="0">'
            f'<stop offset="0" stop-color="{VIOLET}"/><stop offset="0.5" stop-color="{BLEND_MID}"/>'
            f'<stop offset="1" stop-color="{AZURE}"/></linearGradient>')
    body = []
    body.append(f'  <rect x="0" y="0" width="{W}" height="3" fill="url(#seam)"/>')
    body.append(mark(96, 108, 104, "h"))
    body.append(wm)
    body.append(text(252, 224, esc("a symphony of models · many voices, one score"),
                     fill=TX2, size=21, weight=500))
    body.append(text(252, 258,
                     f'<tspan fill="{V_CLAUDE}">builder</tspan> writes  ·  '
                     f'<tspan fill="{V_CODEX}">critic</tspan> from a different lab reviews  ·  '
                     f'they iterate until they’re <tspan fill="{OK}">in harmony</tspan>',
                     fill=TX_MUTE, size=16, weight=400))
    body.append(text(96, 296, "♪ ♫ ♬   adversarial, cross-model AI development for the command line",
                     fill=TX_MUTE, size=13))
    body.append(equalizer(880, 210, 28, 8, 5, 110, animate=True))
    write("duet-hero.svg", svg(W, H, "\n".join(body), defs, rounded=18))


# ── 2. terminal mock (the live shell) ───────────────────────────────────────
def shell():
    W, H = 1040, 668
    PX, PW = 28, 984
    rows_x = PX + 22
    y = 132
    LH = 30
    body = []
    # window frame
    body.append(f'  <rect x="{PX}" y="20" width="{PW}" height="{H-40}" rx="14" fill="{BG}" '
                f'stroke="{B_SUBTLE}" stroke-width="1"/>')
    # title chrome
    body.append(f'  <line x1="{PX}" y1="66" x2="{PX+PW}" y2="66" stroke="{B_SUBTLE}" stroke-width="1"/>')
    for i, dot in enumerate([B_STRONG, B_STRONG, B_STRONG]):
        body.append(f'  <circle cx="{PX+26+i*22}" cy="43" r="6" fill="{dot}"/>')
    body.append(text(PX + PW / 2, 48, "duet — adversarial development", fill=TX_MUTE, size=13, anchor="middle"))
    # header row: badge + ensemble + ready
    hy = 104
    body.append(f'  <rect x="{rows_x}" y="{hy-18}" width="74" height="24" rx="5" fill="{PERIWINKLE}"/>')
    body.append(text(rows_x + 12, hy - 1, "♫ duet", fill=BG, size=15, weight=700))
    body.append(text(rows_x + 92, hy - 1,
                     f'<tspan fill="{V_CLAUDE}">claude</tspan>'
                     f'<tspan fill="{TX_MUTE}"> ⇄ </tspan>'
                     f'<tspan fill="{V_CODEX}">codex</tspan>'
                     f'<tspan fill="{TX_FAINT}">  ·  </tspan><tspan fill="{TX2}">code</tspan>'
                     f'<tspan fill="{TX_FAINT}">  ·  </tspan><tspan fill="{TX2}">3 rounds</tspan>',
                     fill=TX2, size=15))
    body.append(text(PX + PW - 24, hy - 1, "♪ ready", fill=TX_MUTE, size=14, anchor="end"))

    def phase(label, model, mc):
        nonlocal y
        body.append(text(rows_x, y,
                         f'<tspan fill="{TX_FAINT}">──  </tspan>'
                         f'<tspan fill="{TX2}" font-weight="700">{label}</tspan>'
                         f'<tspan fill="{TX_FAINT}"> · </tspan>'
                         f'<tspan fill="{mc}">{model}</tspan>'
                         f'<tspan fill="{TX_FAINT}">  ' + "─" * 46 + '</tspan>',
                         fill=TX_FAINT, size=15))
        y += LH

    def row(model, mc, glyph, gcol, content):
        nonlocal y
        body.append(text(rows_x, y,
                         f'<tspan fill="{mc}">┃ </tspan>'
                         f'<tspan fill="{mc}">{model:<6}</tspan>'
                         f'<tspan fill="{gcol}"> {glyph} </tspan>'
                         f'<tspan> </tspan>{content}',
                         fill=TX, size=15))
        y += LH

    y = 152
    phase("Build", "claude", V_CLAUDE)
    row("claude", V_CLAUDE, "⚙", V_CLAUDE, f'Edit  <tspan fill="{F_PY}">●</tspan> <tspan fill="{TX}">stats.py</tspan>')
    row("claude", V_CLAUDE, "⚙", V_CLAUDE, f'<tspan fill="{TX}">pytest -q</tspan>  <tspan fill="{OK}">(exit 0)</tspan>')
    row("claude", V_CLAUDE, "✎", OK, f'<tspan fill="{F_PY}">●</tspan> <tspan fill="{TX}">stats.py</tspan>  <tspan fill="{F_MD}">●</tspan> <tspan fill="{TX}">README.md</tspan>')
    phase("Review", "codex", V_CODEX)
    row("codex", V_CODEX, "┃", V_CODEX, f'<tspan fill="{TX}">solid — one edge case: median() divides by zero on empty input</tspan>')
    body.append(text(rows_x, y,
                     f'<tspan fill="{ERR}" font-weight="700">[major] </tspan>'
                     f'<tspan fill="{TX2}">stats.py:14 — guard the empty slice before the divide</tspan>',
                     fill=TX2, size=15))
    y += LH
    phase("Build", "claude", V_CLAUDE)
    row("claude", V_CLAUDE, "┃", V_CLAUDE, f'<tspan fill="{TX}">added the guard + a regression test; re-running the gate</tspan>')
    body.append(text(rows_x, y,
                     f'<tspan fill="{OK}" font-weight="700">♫ in harmony</tspan>'
                     f'<tspan fill="{TX_MUTE}"> — converged ♪</tspan>',
                     fill=OK, size=15))

    # equalizer strip
    body.append(equalizer(rows_x, 556, 44, 7, 5, 30, animate=True))
    # input box
    iy = 576
    body.append(f'  <rect x="{rows_x}" y="{iy}" width="{PW-44}" height="40" rx="8" '
                f'fill="{INSET}" stroke="{PERIWINKLE}" stroke-opacity="0.55" stroke-width="1"/>')
    body.append(text(rows_x + 16, iy + 26,
                     f'<tspan fill="{V_CLAUDE}">♪ code ▸ </tspan>'
                     f'<tspan fill="{TX_MUTE}">add a median() with an empty-input guard and tests</tspan>',
                     fill=TX, size=15))
    body.append(text(rows_x, iy + 62,
                     "just type to chat  ·  '/' for commands  ·  /run &lt;task&gt; for the full workflow  ·  /quit",
                     fill=TX_MUTE, size=13))
    write("duet-shell.svg", svg(W, H, "\n".join(body)))


# ── 3. loop diagram ─────────────────────────────────────────────────────────
def loop():
    W, H = 1280, 300
    cy = 150
    body = []
    body.append(text(64, 60, "how it works", fill=TX2, size=18, weight=700))
    body.append(text(64, 86, "plan, then build and review in a tight adversarial loop, then verify against an objective gate",
                     fill=TX_MUTE, size=14))

    def chip(x, w, title, sub, color, fill):
        body.append(f'  <rect x="{x}" y="{cy-44}" width="{w}" height="88" rx="12" fill="{fill}" '
                    f'stroke="{color}" stroke-width="1.5"/>')
        body.append(text(x + w / 2, cy - 4, title, fill=color, size=20, weight=700, anchor="middle"))
        body.append(text(x + w / 2, cy + 24, sub, fill=TX_MUTE, size=13, anchor="middle"))

    def arrow(x1, x2, label="", color=PERIWINKLE):
        body.append(f'  <line x1="{x1}" y1="{cy}" x2="{x2-10}" y2="{cy}" stroke="{color}" stroke-width="2"/>')
        body.append(f'  <path d="M{x2-10} {cy-5} L{x2} {cy} L{x2-10} {cy+5} Z" fill="{color}"/>')
        if label:
            body.append(text((x1 + x2) / 2, cy - 16, label, fill=TX_MUTE, size=12, anchor="middle"))

    chip(64, 150, "plan", "+ red-team", PERIWINKLE, "rgba(135,135,255,0.10)")
    arrow(214, 286)
    chip(286, 196, "Build", "builder writes", VIOLET, "rgba(168,132,255,0.12)")
    # build <-> review pairing
    body.append(text(286 + 196 + 36, cy + 5, "⇄", fill=PERIWINKLE, size=30, weight=700, anchor="middle"))
    body.append(text(286 + 196 + 36, cy - 28, "fix", fill=TX_MUTE, size=12, anchor="middle"))
    body.append(text(286 + 196 + 36, cy + 40, "review", fill=TX_MUTE, size=12, anchor="middle"))
    chip(286 + 196 + 72, 196, "Review", "critic objects", AZURE, "rgba(58,160,255,0.12)")
    arrow(286 + 196 + 72 + 196, 850)
    chip(850, 150, "verify", "objective gate", V_LOCAL, "rgba(0,215,215,0.10)")
    arrow(1000, 1040)
    # harmony terminal
    body.append(f'  <rect x="1040" y="{cy-44}" width="176" height="88" rx="12" '
                f'fill="rgba(63,185,80,0.10)" stroke="{OK}" stroke-width="1.5"/>')
    body.append(text(1040 + 88, cy - 4, "♫ in harmony", fill=OK, size=18, weight=700, anchor="middle"))
    body.append(text(1040 + 88, cy + 24, "converged ♪", fill=TX_MUTE, size=13, anchor="middle"))
    write("duet-loop.svg", svg(W, H, "\n".join(body)))


# ── 4. social / OG card ─────────────────────────────────────────────────────
def social():
    W, H = 1280, 640
    g, _ = wordmark(W / 2, 372, 150, "s")
    defs = g
    body = []
    body.append(f'  <rect x="0" y="0" width="{W}" height="4" fill="{PERIWINKLE}"/>')
    body.append(mark(W / 2 - 64, 150, 128, "s"))
    # center the wordmark
    body.append(f'<text x="{W/2}" y="372" fill="url(#wms)" font-size="150" font-weight="800" '
                f'letter-spacing="-3" text-anchor="middle">duet</text>')
    body.append(text(W / 2, 430, esc("a symphony of models · many voices, one score"),
                     fill=TX2, size=27, weight=500, anchor="middle"))
    body.append(text(W / 2, 470,
                     f'one model writes · a model from a <tspan fill="{V_CODEX}">different lab</tspan> reviews · '
                     f'until they’re <tspan fill="{OK}">in harmony</tspan>',
                     fill=TX_MUTE, size=18, anchor="middle"))
    body.append(equalizer(W / 2 - 198, 548, 33, 8, 4, 70, animate=False))
    body.append(text(W / 2, 600, "github.com/cyberslop/duet", fill=TX_MUTE, size=15, anchor="middle"))
    write("duet-social.svg", svg(W, H, "\n".join(body), defs))


if __name__ == "__main__":
    print("generating duet brand graphics →", HERE)
    hero()
    shell()
    loop()
    social()
    print("done.")
