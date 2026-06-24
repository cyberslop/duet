//! Prompt templates as *data*. Defaults are embedded at build time, but any
//! template can be overridden at runtime by setting `DUET_PROMPTS=/dir` with a
//! matching `<name>.txt` (so prompts stay tunable without a rebuild). Templates
//! use `{{TASK}}`, `{{BUILDER}}`, `{{CRITIC}}`, `{{TEST}}`, `{{ROUND}}`.

use std::path::Path;

const PLAN: &str = include_str!("../prompts/plan.txt");
const PLAN_REVIEW: &str = include_str!("../prompts/plan_review.txt");
const PLAN_REVISE: &str = include_str!("../prompts/plan_revise.txt");
const IMPLEMENT: &str = include_str!("../prompts/implement.txt");
const REVIEW: &str = include_str!("../prompts/review.txt");
const ADDRESS: &str = include_str!("../prompts/address.txt");
const CONDUCTOR_STRATEGY: &str = include_str!("../prompts/conductor_strategy.txt");
const CONDUCTOR_PROGRESS: &str = include_str!("../prompts/conductor_progress.txt");
const CONDUCTOR_HANDOFF: &str = include_str!("../prompts/conductor_handoff.txt");

fn load(name: &str, default: &str) -> String {
    if let Some(dir) = std::env::var_os("DUET_PROMPTS") {
        let p = Path::new(&dir).join(format!("{name}.txt"));
        if let Ok(s) = std::fs::read_to_string(p) {
            return s;
        }
    }
    default.to_string()
}

fn fill(tpl: &str, subs: &[(&str, &str)]) -> String {
    let mut out = tpl.to_string();
    for (k, v) in subs {
        let needle = ["{{", k, "}}"].concat();
        out = out.replace(&needle, v);
    }
    out
}

pub fn plan(task: &str, builder: &str, critic: &str) -> String {
    fill(&load("plan", PLAN), &[("TASK", task), ("BUILDER", builder), ("CRITIC", critic)])
}
pub fn plan_review(task: &str, builder: &str, critic: &str) -> String {
    fill(&load("plan_review", PLAN_REVIEW), &[("TASK", task), ("BUILDER", builder), ("CRITIC", critic)])
}
pub fn plan_revise(builder: &str) -> String {
    fill(&load("plan_revise", PLAN_REVISE), &[("BUILDER", builder)])
}
pub fn implement(builder: &str, test: &str) -> String {
    fill(&load("implement", IMPLEMENT), &[("BUILDER", builder), ("TEST", test)])
}
pub fn review(task: &str, builder: &str, critic: &str, round: usize) -> String {
    fill(
        &load("review", REVIEW),
        &[("TASK", task), ("BUILDER", builder), ("CRITIC", critic), ("ROUND", &round.to_string())],
    )
}
pub fn address(builder: &str, critic: &str, test: &str, round: usize) -> String {
    fill(
        &load("address", ADDRESS),
        &[("BUILDER", builder), ("CRITIC", critic), ("TEST", test), ("ROUND", &round.to_string())],
    )
}

// ── conductor mode (strategist directs implementer over a long horizon) ──

pub fn conductor_strategy(task: &str, strategist: &str, implementer: &str) -> String {
    fill(
        &load("conductor_strategy", CONDUCTOR_STRATEGY),
        &[("TASK", task), ("STRATEGIST", strategist), ("IMPLEMENTER", implementer)],
    )
}
pub fn conductor_progress(task: &str, round: usize) -> String {
    fill(&load("conductor_progress", CONDUCTOR_PROGRESS), &[("TASK", task), ("ROUND", &round.to_string())])
}
pub fn conductor_handoff(objective: &str, test: &str, round: usize) -> String {
    fill(
        &load("conductor_handoff", CONDUCTOR_HANDOFF),
        &[("OBJECTIVE", objective), ("TEST", test), ("ROUND", &round.to_string())],
    )
}
