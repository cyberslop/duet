//! duet's design-system palette as ratatui colors — one source of truth for the
//! shell and the run viewer. On terminals that advertise 24-bit color
//! (`COLORTERM=truecolor|24bit`) the exact brand hex from `tokens/colors.css` is
//! used; otherwise the ANSI-256 index the token was sampled from stands in, so
//! the UI stays on-brand on terminals without truecolor (e.g. macOS Terminal).

use ratatui::style::Color;
use std::sync::OnceLock;

fn truecolor() -> bool {
    static TC: OnceLock<bool> = OnceLock::new();
    *TC.get_or_init(|| {
        matches!(
            std::env::var("COLORTERM").ok().as_deref(),
            Some("truecolor") | Some("24bit")
        )
    })
}

/// Exact hex on truecolor terminals; the given ANSI-256 index otherwise.
fn c(idx: u8, r: u8, g: u8, b: u8) -> Color {
    if truecolor() {
        Color::Rgb(r, g, b)
    } else {
        Color::Indexed(idx)
    }
}

// ── Voices (model roles) ────────────────────────────────────────────────────
pub fn claude() -> Color {
    c(141, 0xAF, 0x87, 0xFF)
}
pub fn codex() -> Color {
    c(39, 0x00, 0xAF, 0xFF)
}
pub fn local() -> Color {
    c(44, 0x00, 0xD7, 0xD7)
}

// ── Brand ───────────────────────────────────────────────────────────────────
pub fn violet() -> Color {
    c(141, 0xA8, 0x84, 0xFF)
}
pub fn periwinkle() -> Color {
    c(105, 0x87, 0x87, 0xFF)
}
/// Charcoal-navy ground — used as the text color on a periwinkle badge.
pub fn on_accent() -> Color {
    c(0, 0x15, 0x17, 0x1F)
}

// ── Semantics ───────────────────────────────────────────────────────────────
pub fn ok() -> Color {
    c(40, 0x3F, 0xB9, 0x50)
}
pub fn warn() -> Color {
    c(178, 0xD2, 0x99, 0x22)
}
pub fn err() -> Color {
    c(203, 0xF8, 0x51, 0x49)
}

// ── Text ────────────────────────────────────────────────────────────────────
pub fn text() -> Color {
    c(255, 0xEC, 0xEE, 0xF6)
}
pub fn secondary() -> Color {
    c(250, 0xAA, 0xB0, 0xC2)
}
pub fn muted() -> Color {
    c(244, 0x6B, 0x72, 0x82)
}
pub fn faint() -> Color {
    c(240, 0x45, 0x4B, 0x5C)
}

// ── Structure ───────────────────────────────────────────────────────────────
pub fn border() -> Color {
    c(236, 0x26, 0x2B, 0x3A)
}

// ── Severity (review findings) ──────────────────────────────────────────────
pub fn severity(sev: &str) -> Color {
    match sev {
        "blocker" | "major" => err(),
        "minor" => warn(),
        _ => muted(),
    }
}

// ── File-type accents (per-extension dot colors) ────────────────────────────
pub fn file(ext: &str) -> Color {
    match ext {
        "rs" => c(208, 0xFF, 0x87, 0x00),
        "py" => c(33, 0x00, 0x87, 0xFF),
        "md" | "markdown" => c(75, 0x5F, 0xAF, 0xFF),
        "json" | "jsonl" => c(178, 0xD7, 0xAF, 0x00),
        "go" => c(44, 0x00, 0xD7, 0xD7),
        "sh" | "bash" | "zsh" => c(120, 0x87, 0xFF, 0x87),
        "js" | "ts" | "tsx" => c(227, 0xFF, 0xFF, 0x5F),
        "diff" | "patch" => c(168, 0xD7, 0x5F, 0x87),
        _ => c(250, 0xBC, 0xBC, 0xBC),
    }
}

/// The launch-equalizer wave, colored blue → periwinkle → violet by amplitude.
pub fn eq_tier(height: usize) -> Color {
    match height {
        0..=2 => codex(),      // blue (quiet)
        3..=5 => periwinkle(), // periwinkle (mid)
        _ => violet(),         // violet (peak)
    }
}
