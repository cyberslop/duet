//! Profiles: named, per-use-case bindings of roles to models, as data. Built-in
//! defaults are embedded; users add/override via `$DUET_PROFILES` or
//! `~/.config/duet/profiles.toml` (same name wins). This is the routing layer —
//! "local for bulk rounds, frontier for the final gate" is a TOML edit, not code.

use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::path::PathBuf;

const DEFAULTS: &str = include_str!("profiles.toml");

#[derive(Deserialize, Clone)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "d_claude")]
    pub builder: String,
    #[serde(default = "d_codex")]
    pub critic: String,
    #[serde(default = "d_rounds")]
    pub rounds: usize,
    #[serde(default)]
    pub swap: bool,
    #[serde(default)]
    pub no_plan: bool,
    #[serde(default)]
    pub test_cmd: Option<String>,
    #[serde(default = "d_code")]
    pub domain: String,
    #[serde(default)]
    pub local_endpoint: Option<String>,
    #[serde(default)]
    pub local_model: Option<String>,
    /// Conductor mode: `builder` is the implementer and `strategist` directs it.
    #[serde(default)]
    pub conductor: bool,
    #[serde(default)]
    pub strategist: Option<String>,
}

fn d_claude() -> String {
    "claude".into()
}
fn d_codex() -> String {
    "codex".into()
}
fn d_rounds() -> usize {
    3
}
fn d_code() -> String {
    "code".into()
}

#[derive(Deserialize, Default)]
struct ProfileFile {
    #[serde(default)]
    profile: Vec<Profile>,
}

fn parse(s: &str) -> Vec<Profile> {
    toml::from_str::<ProfileFile>(s).map(|f| f.profile).unwrap_or_default()
}

fn user_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("DUET_PROFILES") {
        return Some(PathBuf::from(p));
    }
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("duet/profiles.toml"))
}

/// Built-in defaults plus any user profiles (user wins on name collision).
pub fn load_all() -> Vec<Profile> {
    let mut profiles = parse(DEFAULTS);
    if let Some(p) = user_path() {
        if let Ok(s) = std::fs::read_to_string(&p) {
            match toml::from_str::<ProfileFile>(&s) {
                Ok(f) => {
                    for up in f.profile {
                        match profiles.iter_mut().find(|x| x.name == up.name) {
                            Some(existing) => *existing = up,
                            None => profiles.push(up),
                        }
                    }
                }
                Err(e) => eprintln!("warning: ignoring {} ({e})", p.display()),
            }
        }
    }
    profiles
}

pub fn find(name: &str) -> Result<Profile> {
    let all = load_all();
    all.iter().find(|p| p.name == name).cloned().ok_or_else(|| {
        let names = all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ");
        anyhow!("unknown profile '{name}'. Available: {names}")
    })
}

#[cfg(test)]
mod tests {
    use super::parse;
    const DEFAULTS: &str = include_str!("profiles.toml");

    #[test]
    fn builtin_profiles_parse() {
        let ps = parse(DEFAULTS);
        assert!(ps.iter().any(|p| p.name == "code-frontier"));
        // Non-conductor profiles default to conductor=false with no strategist.
        let cf = ps.iter().find(|p| p.name == "code-frontier").unwrap();
        assert!(!cf.conductor);
        assert!(cf.strategist.is_none());
    }

    #[test]
    fn conductor_profiles_are_well_formed() {
        let ps = parse(DEFAULTS);
        let conductors: Vec<_> = ps.iter().filter(|p| p.conductor).collect();
        assert!(conductors.len() >= 2, "expected both cross-vendor conductor profiles");
        for p in conductors {
            let strat = p.strategist.as_deref().expect("a conductor profile names a strategist");
            // The whole point is a cross-model pairing: strategist != implementer.
            assert_ne!(strat, p.builder, "profile {}: strategist and implementer must differ", p.name);
            assert!(matches!(strat, "claude" | "codex"), "profile {}: strategist must have file tools", p.name);
        }
    }
}
