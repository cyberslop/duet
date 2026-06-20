//! Minimal ANSI theme (no external color crate). Colors auto-disable when stdout
//! is not a TTY or when `NO_COLOR` is set. On terminals that advertise truecolor
//! (`COLORTERM=truecolor|24bit`) the exact duet brand hex is used; otherwise the
//! ANSI-256 approximations the design tokens were derived from stand in.

use std::io::IsTerminal;

#[derive(Clone, Copy)]
pub struct Theme {
    pub rst: &'static str,
    pub dim: &'static str,
    pub bold: &'static str,
    pub claude: &'static str,
    pub codex: &'static str,
    pub local: &'static str,
    pub ok: &'static str,
    pub warn: &'static str,
    pub err: &'static str,
}

impl Theme {
    pub fn detect() -> Self {
        if !std::io::stdout().is_terminal() || std::env::var_os("NO_COLOR").is_some() {
            Self::plain()
        } else if truecolor() {
            Self::truecolor()
        } else {
            Self::color()
        }
    }

    /// Exact brand hex (24-bit) — voices and semantics match `tokens/colors.css`.
    pub fn truecolor() -> Self {
        Theme {
            rst: "\x1b[0m",
            dim: "\x1b[2m",
            bold: "\x1b[1m",
            claude: "\x1b[38;2;175;135;255m", // #AF87FF
            codex: "\x1b[38;2;0;175;255m",    // #00AFFF
            local: "\x1b[38;2;0;215;215m",    // #00D7D7
            ok: "\x1b[38;2;63;185;80m",       // #3FB950
            warn: "\x1b[38;2;210;153;34m",    // #D29922
            err: "\x1b[38;2;248;81;73m",      // #F85149
        }
    }

    /// ANSI-256 fallback (the indices the brand hex was sampled from).
    pub fn color() -> Self {
        Theme {
            rst: "\x1b[0m",
            dim: "\x1b[2m",
            bold: "\x1b[1m",
            claude: "\x1b[38;5;141m",
            codex: "\x1b[38;5;39m",
            local: "\x1b[38;5;44m",
            ok: "\x1b[38;5;40m",
            warn: "\x1b[38;5;178m",
            err: "\x1b[38;5;203m",
        }
    }

    pub fn plain() -> Self {
        Theme { rst: "", dim: "", bold: "", claude: "", codex: "", local: "", ok: "", warn: "", err: "" }
    }
}

/// Whether the terminal advertises 24-bit color via `COLORTERM`.
fn truecolor() -> bool {
    matches!(std::env::var("COLORTERM").ok().as_deref(), Some("truecolor") | Some("24bit"))
}
