//! A curated catalog of strong open-weight models for local inference, tagged by
//! task. These are priors (knowledge cutoff): the ModelAdvisor can refresh them
//! with live research, but the catalog gives a useful, offline baseline so
//! `duet suggest-models` works without any network or API call.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Code,
    Reasoning,
    Research,
    Security,
    Data,
    Triage,
    General,
}

impl Task {
    pub fn parse(s: &str) -> Option<Task> {
        Some(match s.to_lowercase().as_str() {
            "code" | "coding" => Task::Code,
            "reasoning" | "reason" => Task::Reasoning,
            "research" => Task::Research,
            "security" | "sec" => Task::Security,
            "data" | "analysis" => Task::Data,
            "triage" => Task::Triage,
            "general" => Task::General,
            _ => return None,
        })
    }
}

/// Quantization level → approximate bytes/parameter (weights only).
#[derive(Debug, Clone, Copy)]
pub enum Quant {
    Q4KM,
    Q5KM,
    Q8,
}

impl Quant {
    pub fn bytes_per_param(self) -> f64 {
        match self {
            Quant::Q4KM => 0.56,
            Quant::Q5KM => 0.68,
            Quant::Q8 => 1.06,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Quant::Q4KM => "Q4_K_M",
            Quant::Q5KM => "Q5_K_M",
            Quant::Q8 => "Q8_0",
        }
    }
    pub fn all() -> [Quant; 3] {
        [Quant::Q8, Quant::Q5KM, Quant::Q4KM] // best → smallest
    }
}

#[derive(Debug, Clone)]
pub struct CatalogModel {
    pub name: &'static str,
    /// LM Studio / HuggingFace search id to download.
    pub pull_id: &'static str,
    pub params_b: f64,
    pub tasks: &'static [Task],
    pub note: &'static str,
}

impl CatalogModel {
    /// Approx on-disk / in-memory footprint at a quant, incl. ~1.5 GB runtime/KV overhead.
    ///
    /// MoE models ship a fixed low-bit weight format and load far smaller than the
    /// dense `params_b * bytes_per_param` estimate implies, so applying the dense
    /// formula would over-budget them and wrongly exclude models that fit. For the
    /// known MoE entries we use their measured loaded size instead.
    pub fn footprint_gb(&self, q: Quant) -> f64 {
        if let Some(gb) = self.moe_loaded_gb() {
            return gb + 1.0; // measured weights + KV/runtime overhead
        }
        self.params_b * q.bytes_per_param() + 1.5
    }
    /// Measured as-loaded size for MoE models (None = dense, use the formula).
    fn moe_loaded_gb(&self) -> Option<f64> {
        match self.pull_id {
            "openai/gpt-oss-20b" => Some(13.0),
            "qwen/qwen3-next-80b" => Some(45.0),
            "openai/gpt-oss-120b" => Some(63.0),
            _ => None,
        }
    }
    pub fn handles(&self, task: Task) -> bool {
        self.tasks.contains(&task)
    }
}

use Task::*;

/// The curated baseline. Ordered roughly small → large within capability.
pub const CATALOG: &[CatalogModel] = &[
    CatalogModel { name: "Qwen2.5-Coder-3B-Instruct", pull_id: "qwen2.5-coder-3b-instruct", params_b: 3.0, tasks: &[Code, Triage], note: "tiny, fast first-pass code triage" },
    CatalogModel { name: "Llama-3.1-8B-Instruct", pull_id: "llama-3.1-8b-instruct", params_b: 8.0, tasks: &[General, Triage, Research], note: "small general workhorse" },
    CatalogModel { name: "Qwen2.5-Coder-7B-Instruct", pull_id: "qwen2.5-coder-7b-instruct", params_b: 7.0, tasks: &[Code, Triage], note: "strong small code reviewer" },
    CatalogModel { name: "Phi-4", pull_id: "phi-4", params_b: 14.0, tasks: &[Reasoning, Triage], note: "punches above its size on reasoning" },
    CatalogModel { name: "Qwen2.5-Coder-14B-Instruct", pull_id: "qwen2.5-coder-14b-instruct", params_b: 14.0, tasks: &[Code, Triage], note: "mid code critic, good cost/quality" },
    CatalogModel { name: "Codestral-22B", pull_id: "codestral-22b-v0.1", params_b: 22.0, tasks: &[Code], note: "code-specialized, broad language support" },
    CatalogModel { name: "Mistral-Small-24B-Instruct", pull_id: "mistral-small-24b-instruct-2501", params_b: 24.0, tasks: &[General, Research, Reasoning], note: "well-rounded mid model" },
    CatalogModel { name: "Gemma-2-27B-Instruct", pull_id: "gemma-2-27b-it", params_b: 27.0, tasks: &[General, Research], note: "strong summarization/research" },
    CatalogModel { name: "Qwen2.5-Coder-32B-Instruct", pull_id: "qwen2.5-coder-32b-instruct", params_b: 32.0, tasks: &[Code, Reasoning, Security], note: "best local code reviewer; near-frontier on review tasks" },
    CatalogModel { name: "DeepSeek-R1-Distill-Qwen-32B", pull_id: "deepseek-r1-distill-qwen-32b", params_b: 32.0, tasks: &[Reasoning, Security], note: "explicit chain-of-thought; great adversarial critic" },
    CatalogModel { name: "Llama-3.3-70B-Instruct", pull_id: "llama-3.3-70b-instruct", params_b: 70.0, tasks: &[Reasoning, Research, General, Security], note: "frontier-adjacent reasoning when it fits" },
    CatalogModel { name: "Qwen2.5-72B-Instruct", pull_id: "qwen2.5-72b-instruct", params_b: 72.0, tasks: &[Reasoning, Research, Code], note: "top-tier local generalist" },
    // 2026-era MoE models (memory ≈ as-loaded; MoE so lighter than dense param count).
    CatalogModel { name: "gpt-oss-20b", pull_id: "openai/gpt-oss-20b", params_b: 20.0, tasks: &[Reasoning, Code, Triage], note: "OpenAI open-weight MoE; fast, strong reasoning for its size" },
    CatalogModel { name: "Qwen3-Next-80B", pull_id: "qwen/qwen3-next-80b", params_b: 80.0, tasks: &[Code, Reasoning, Research, General], note: "MoE ~45 GB loaded; excellent local code reviewer (long context)" },
    CatalogModel { name: "gpt-oss-120b", pull_id: "openai/gpt-oss-120b", params_b: 120.0, tasks: &[Reasoning, Research, Security, General], note: "OpenAI open-weight MoE ~63 GB; near-frontier reasoning when it fits" },
];
