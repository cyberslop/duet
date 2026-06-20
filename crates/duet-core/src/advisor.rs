//! ModelAdvisor: given the probed hardware and a task, recommend which LOCAL
//! model to run — fitted to the device's memory budget, ranked by capability,
//! degrading honestly to "route to cloud" when nothing capable fits.

use crate::catalog::{Quant, Task, CATALOG};
use crate::hardware::Hardware;

#[derive(Debug, Clone)]
pub struct Recommendation {
    pub name: String,
    pub pull_id: String,
    pub quant: Quant,
    pub footprint_gb: f64,
    pub params_b: f64,
    pub note: String,
}

/// Best model for `task` that fits `budget_gb`: the largest model whose best
/// affordable quant fits, preferring a higher quant on a given model.
fn best_fit(task: Task, budget_gb: f64) -> Option<Recommendation> {
    let mut best: Option<Recommendation> = None;
    for m in CATALOG.iter().filter(|m| m.handles(task)) {
        for q in Quant::all() {
            let fp = m.footprint_gb(q);
            if fp <= budget_gb {
                let cand = Recommendation {
                    name: m.name.into(),
                    pull_id: m.pull_id.into(),
                    quant: q,
                    footprint_gb: fp,
                    params_b: m.params_b,
                    note: m.note.into(),
                };
                // prefer more params; tie-break on higher quant (lower fp loses tie)
                best = match best {
                    Some(b) if b.params_b > cand.params_b => Some(b),
                    Some(b) if (b.params_b - cand.params_b).abs() < f64::EPSILON && b.footprint_gb >= cand.footprint_gb => Some(b),
                    _ => Some(cand),
                };
                break; // best (highest) quant that fits this model; move to next model
            }
        }
    }
    best
}

/// A tiered recommendation for a task: a capable pick and a fast/cheap pick.
#[derive(Debug, Clone)]
pub struct Advice {
    pub task: Task,
    pub capable: Option<Recommendation>,
    pub fast: Option<Recommendation>,
}

pub fn advise(hw: &Hardware, task: Task) -> Advice {
    let budget = hw.usable_model_gb;
    let capable = best_fit(task, budget);
    // "fast" = best model that fits in a tight budget (<= ~12 GB) so it stays snappy
    let fast = best_fit(task, budget.min(12.0)).or_else(|| {
        // include triage-tagged small models too
        best_fit(Task::Triage, budget.min(12.0))
    });
    Advice { task, capable, fast }
}

/// Map a duet domain to the local tasks worth recommending models for.
pub fn tasks_for_domain(domain: &str) -> Vec<Task> {
    match domain {
        "research" => vec![Task::Research, Task::Reasoning],
        "security" => vec![Task::Security, Task::Code, Task::Reasoning],
        _ => vec![Task::Code, Task::Reasoning], // code (default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{Accelerator, Hardware};

    fn hw(gb: f64) -> Hardware {
        Hardware { os: "test".into(), arch: "aarch64".into(), chip: "test".into(), total_ram_gb: gb / 0.7, usable_model_gb: gb, accelerator: Accelerator::Metal }
    }

    #[test]
    fn beefy_box_gets_a_large_coder() {
        let a = advise(&hw(90.0), Task::Code);
        let cap = a.capable.expect("a capable code model fits in 90 GB");
        assert!(cap.params_b >= 32.0, "should pick a large coder, got {}B", cap.params_b);
    }

    #[test]
    fn small_box_degrades_to_a_small_model_or_none() {
        let a = advise(&hw(6.0), Task::Code);
        // 6 GB budget: only the tiny coders fit (or nothing)
        if let Some(c) = a.capable {
            assert!(c.footprint_gb <= 6.0);
            assert!(c.params_b <= 8.0);
        }
    }

    #[test]
    fn tiny_box_recommends_cloud() {
        let a = advise(&hw(2.0), Task::Code);
        assert!(a.capable.is_none(), "2 GB can't host a useful coder → route to cloud");
    }
}
