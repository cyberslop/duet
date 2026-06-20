//! Minimal ANSI theme (no external color crate). Colors auto-disable when stdout
//! is not a TTY or when `NO_COLOR` is set.

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
        if std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
            Self::color()
        } else {
            Self::plain()
        }
    }

    pub fn color() -> Self {
        Theme {
            rst: "\x1b[0m",
            dim: "\x1b[2m",
            bold: "\x1b[1m",
            claude: "\x1b[38;5;141m",
            codex: "\x1b[38;5;39m",
            local: "\x1b[38;5;44m",
            ok: "\x1b[32m",
            warn: "\x1b[33m",
            err: "\x1b[31m",
        }
    }

    pub fn plain() -> Self {
        Theme { rst: "", dim: "", bold: "", claude: "", codex: "", local: "", ok: "", warn: "", err: "" }
    }
}
