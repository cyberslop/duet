//! Domains make the engine generic. The engine owns the skeleton
//! (plan → build → review⇄fix → final → verify); a `Domain` supplies the
//! variable content: what the builder produces, what the critic reviews, the
//! review schema, and — critically — an OBJECTIVE `verify()` gate (never LLM
//! self-grading). `CodeDomain` is today's behavior verbatim; `ResearchDomain`
//! runs a gather → verify-claims-against-sources workflow.

use crate::{git, prompts, REVIEW_SCHEMA};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// The objective convergence gate result. Three-valued so an ungated run is
/// visibly `Skipped`, never falsely "converged".
pub enum Convergence {
    Passed,
    Failed(String),
    Skipped,
}

pub trait Domain: Send + Sync {
    fn name(&self) -> &str;
    /// Noun for the artifact under review ("diff", "report") — labels & filenames.
    fn unit_kind(&self) -> &str;

    // ── builder-side prompts ──
    fn plan_prompt(&self, task: &str, bl: &str, cl: &str) -> String;
    fn plan_revise_prompt(&self, bl: &str) -> String;
    fn build_prompt(&self, task: &str, bl: &str) -> String;
    fn fix_prompt(&self, task: &str, bl: &str, cl: &str, round: usize) -> String;

    // ── critic-side framing ──
    fn plan_review_prompt(&self, task: &str, bl: &str, cl: &str) -> String;
    fn review_prompt(&self, task: &str, bl: &str, cl: &str, round: usize) -> String;
    fn review_schema(&self) -> &'static str {
        REVIEW_SCHEMA
    }
    /// Extra allowed tools a CLI critic needs (empty for code → parity).
    fn critic_tools(&self) -> &'static str {
        ""
    }
    /// (system, user) for a tool-less local critic reviewing the inlined unit.
    fn local_review_framing(&self, unit_text: &str) -> (String, String);
    /// (system, user) for a tool-less local critic red-teaming the inlined plan.
    fn local_redteam_framing(&self, plan_text: &str) -> (String, String) {
        (
            "You are an adversarial reviewer red-teaming a plan before any work is done.".into(),
            format!("Red-team this plan: wrong assumptions, missing edge cases, simpler/safer alternatives, untestable parts, scope gaps. Cite specifics. Output a prioritized markdown critique.\n\n--- PLAN ---\n{plan_text}"),
        )
    }

    /// Capture/refresh the artifact the critic reviews; return its path.
    fn capture_unit(&self, repo: &Path, base: &str, round: usize, duet: &Path) -> Result<PathBuf>;

    /// The objective gate. Must be mechanical and reproducible — the LLM may
    /// narrate the result but never *be* it.
    fn verify(&self, repo: &Path, duet: &Path) -> Convergence;
}

pub fn domain_for(name: &str, test_cmd: String, no_test: bool) -> Box<dyn Domain> {
    match name {
        "research" => Box::new(ResearchDomain),
        "security" => Box::new(SecurityDomain { test_cmd, no_test }),
        _ => Box::new(CodeDomain { test_cmd }),
    }
}

// ───────────────────────────────── code ─────────────────────────────────────

pub struct CodeDomain {
    pub test_cmd: String,
}

impl Domain for CodeDomain {
    fn name(&self) -> &str {
        "code"
    }
    fn unit_kind(&self) -> &str {
        "diff"
    }
    fn plan_prompt(&self, task: &str, bl: &str, cl: &str) -> String {
        prompts::plan(task, bl, cl)
    }
    fn plan_revise_prompt(&self, bl: &str) -> String {
        prompts::plan_revise(bl)
    }
    fn build_prompt(&self, _task: &str, bl: &str) -> String {
        prompts::implement(bl, &self.test_cmd)
    }
    fn fix_prompt(&self, _task: &str, bl: &str, cl: &str, round: usize) -> String {
        prompts::address(bl, cl, &self.test_cmd, round)
    }
    fn plan_review_prompt(&self, task: &str, bl: &str, cl: &str) -> String {
        prompts::plan_review(task, bl, cl)
    }
    fn review_prompt(&self, task: &str, bl: &str, cl: &str, round: usize) -> String {
        prompts::review(task, bl, cl, round)
    }
    fn local_review_framing(&self, unit_text: &str) -> (String, String) {
        (
            "You are an adversarial code reviewer from a different lab than the author. Find real, line-citable bugs, security holes, missing or weak tests, and unhandled edge cases. Prefer a few high-confidence findings over many weak ones.".into(),
            format!("Review the diff below. Output ONLY a JSON object matching this schema (no prose, no fences):\n{REVIEW_SCHEMA}\n\nverdict is \"request_changes\" if any blocker or major exists, else \"approve\".\n\n--- DIFF ---\n{unit_text}"),
        )
    }
    fn capture_unit(&self, repo: &Path, base: &str, round: usize, duet: &Path) -> Result<PathBuf> {
        let diff = duet.join(format!("round-{round}.diff"));
        git::capture_diff(repo, base, &diff)?;
        Ok(diff)
    }
    fn verify(&self, repo: &Path, duet: &Path) -> Convergence {
        if self.test_cmd.is_empty() {
            return Convergence::Skipped;
        }
        match git::run_test(repo, &self.test_cmd, &duet.join("transcript.log")) {
            Ok(true) => Convergence::Passed,
            Ok(false) => Convergence::Failed(format!("tests failed: {}", self.test_cmd)),
            Err(e) => Convergence::Failed(format!("test command error: {e}")),
        }
    }
}

// ─────────────────────────────── research ───────────────────────────────────

pub struct ResearchDomain;

impl Domain for ResearchDomain {
    fn name(&self) -> &str {
        "research"
    }
    fn unit_kind(&self) -> &str {
        "report"
    }
    fn plan_prompt(&self, task: &str, _bl: &str, cl: &str) -> String {
        format!("You are the LEAD RESEARCHER. A different model ({cl}) will adversarially red-team your plan.\n\nQUESTION:\n{task}\n\nWrite a concise research plan to .duet/plan.md: decompose into sub-questions, the sources/angles you'll pursue, what would count as a well-supported answer, and the key risks (bias, stale data, unsupported leaps). Do NOT do the research yet. Output a one-line confirmation.")
    }
    fn plan_revise_prompt(&self, _bl: &str) -> String {
        "Read the red-team in .duet/plan-review.md and update .duet/plan.md to address valid points (for any you reject, add 'Rejected: <reason>'). Keep it tight. Output a one-line confirmation.".into()
    }
    fn build_prompt(&self, task: &str, _bl: &str) -> String {
        format!("You are the LEAD RESEARCHER. Research this question and write a report. Use web search / fetch tools and your knowledge; prefer primary and recent sources.\n\nQUESTION:\n{task}\n\nDeliverables:\n- Write .duet/report.md: a structured answer where EVERY load-bearing claim has an inline citation as a URL in parentheses, e.g. \"X happened in 2024 (https://example.com/source)\". No uncited claims of fact.\n- Write .duet/sources.json: a JSON array of {{\"url\":..., \"title\":..., \"supports\":\"the claim it backs\"}}.\n- Include a short 'Limitations / open questions' section.\n\nA different model will adversarially verify every claim against its citation next. Output a one-paragraph summary.")
    }
    fn fix_prompt(&self, _task: &str, _bl: &str, _cl: &str, round: usize) -> String {
        format!("The reviewer's findings are in .duet/findings-{round}.json. Revise .duet/report.md (and .duet/sources.json): remove or hedge unsupported/over-stated claims, add citations where missing, add counter-evidence the reviewer flagged. Keep every load-bearing claim cited with an inline URL. If you reject a finding, append 'DISMISSED [<claim>] — <reason>' to .duet/response-{round}.md. Output a one-paragraph summary.")
    }
    fn plan_review_prompt(&self, task: &str, bl: &str, _cl: &str) -> String {
        format!("You are the adversarial CRITIC. The LEAD RESEARCHER ({bl}) wrote a research plan in .duet/plan.md for:\n{task}\n\nRead it and red-team: leading framing/bias, sub-questions missed, source types that would be needed, unfalsifiable goals, ways the conclusion could be cherry-picked. Output a concise prioritized markdown critique.")
    }
    fn review_prompt(&self, task: &str, _bl: &str, _cl: &str, _round: usize) -> String {
        format!("You are an adversarial RESEARCH REVIEWER with web access. Read .duet/report.md and .duet/sources.json. The question was:\n{task}\n\nFor every load-bearing claim, OPEN its cited URL with WebFetch and check the source actually supports it (fetch a few of the most important ones if there are many). Flag, as findings: (a) claims with no citation, (b) claims a cited source does not actually support or contradicts, (c) over-generalizations, (d) missing counter-evidence, (e) stale/weak sources. Map severity: blocker = uncited or contradicted load-bearing claim; major = weakly supported; minor/nit = polish.\n\nOutput ONLY a JSON object matching this schema (no prose, no fences):\n{REVIEW_SCHEMA}\nUse the \"file\" field for the claim/section locus and line 0. verdict is \"request_changes\" if any blocker or major exists, else \"approve\".")
    }
    fn critic_tools(&self) -> &'static str {
        "WebSearch WebFetch"
    }
    fn local_review_framing(&self, unit_text: &str) -> (String, String) {
        (
            "You are a rigorous research reviewer. You have the report and its sources list, but you CANNOT fetch URLs — so judge internal consistency: does each claim match what its sources.json entry says it supports, is every load-bearing claim cited, and are there over-statements? Do not invent agreement; flag what you cannot confirm.".into(),
            format!("Below is the report followed by its sources.json. Flag: uncited load-bearing claims (blocker), claims that don't match their sources.json 'supports' entry (major), over-generalizations and stale/weak sourcing (minor). Output ONLY JSON matching this schema (no prose, no fences):\n{REVIEW_SCHEMA}\nUse \"file\" for the claim locus and line 0.\n\n{unit_text}"),
        )
    }
    fn capture_unit(&self, _repo: &Path, _base: &str, _round: usize, duet: &Path) -> Result<PathBuf> {
        // Bundle report + sources so a tool-less local critic sees BOTH; CLI critics
        // read the originals (and fetch URLs) directly. Empty when no report yet.
        let report = std::fs::read_to_string(duet.join("report.md")).unwrap_or_default();
        let bundle = duet.join("review-unit.md");
        if report.trim().is_empty() {
            std::fs::write(&bundle, "")?;
        } else {
            let sources = std::fs::read_to_string(duet.join("sources.json")).unwrap_or_default();
            std::fs::write(&bundle, format!("--- REPORT (.duet/report.md) ---\n{report}\n\n--- SOURCES (.duet/sources.json) ---\n{sources}"))?;
        }
        Ok(bundle)
    }
    fn verify(&self, _repo: &Path, duet: &Path) -> Convergence {
        // Objective gate: distinct cited sources must be present. This checks
        // citation PRESENCE/density (mechanical, deduped), not claim support — a
        // rigorous version would re-fetch each URL and verify support.
        let text = match std::fs::read_to_string(duet.join("report.md")) {
            Ok(t) => t,
            Err(_) => return Convergence::Skipped,
        };
        if text.trim().len() < 200 {
            return Convergence::Failed("report missing or too short".into());
        }
        let urls: std::collections::HashSet<&str> = text
            .split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == '<' || c == '>')
            .filter(|t| t.starts_with("http://") || t.starts_with("https://"))
            .map(|t| t.trim_end_matches(|c: char| ".,;]".contains(c)))
            .collect();
        let entries = std::fs::read_to_string(duet.join("sources.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.as_array().map(|a| a.len()))
            .unwrap_or(0);
        if urls.len() < 3 {
            return Convergence::Failed(format!("only {} distinct cited source(s) in the report", urls.len()));
        }
        if entries < 3 {
            return Convergence::Failed(format!("sources.json lists only {entries} source(s)"));
        }
        Convergence::Passed
    }
}

// ───────────────── security / forensics / reverse-engineering ───────────────
// Builds analysis HARNESSES & TOOLING for security work — incident forensics,
// malware RE, memory forensics (e.g. Volatility3 over AVML images). A build
// workflow tuned with forensics expertise + operational SAFETY (never detonate
// untrusted samples; preserve evidence integrity). Unit = the harness diff; the
// objective gate is the harness's own smoke test.

pub struct SecurityDomain {
    pub test_cmd: String,
    /// The user explicitly disabled the gate (`--no-test`); do not re-autodetect.
    pub no_test: bool,
}

impl Domain for SecurityDomain {
    fn name(&self) -> &str {
        "security"
    }
    fn unit_kind(&self) -> &str {
        "diff"
    }
    fn plan_prompt(&self, task: &str, _bl: &str, cl: &str) -> String {
        format!("You are a SECURITY/FORENSICS ENGINEER building analysis tooling. A different model ({cl}) will red-team your plan.\n\nTASK:\n{task}\n\nWrite a plan to .duet/plan.md covering: the established tools to build ON rather than reinvent (e.g. Volatility3 for memory images, correct handling of the capture format such as AVML/LiME/raw, and YARA / capa / floss / radare2 / rizin / Ghidra-headless / pefile / LIEF for malware RE); the analysis pipeline (input -> parse -> extract -> report); inputs and outputs; and OPERATIONAL SAFETY — treat every sample/image as untrusted and potentially live malware: never execute a sample outside an explicit isolated sandbox, operate on copies, hash inputs (SHA-256) for chain-of-custody, and avoid network egress from analysis. Do NOT write code yet. Output a one-line confirmation.")
    }
    fn plan_revise_prompt(&self, _bl: &str) -> String {
        "Read .duet/plan-review.md and update .duet/plan.md to address valid points (for rejections add 'Rejected: <reason>'). Output a one-line confirmation.".into()
    }
    fn build_prompt(&self, task: &str, _bl: &str) -> String {
        format!("You are a SECURITY/FORENSICS ENGINEER. Build the harness/tool per the plan in .duet/plan.md.\n\nTASK:\n{task}\n\nRequirements:\n- Build ON established forensics tooling (Volatility3 for memory; correct AVML/LiME/raw handling; YARA/capa/radare2 for malware RE) rather than reinventing parsers.\n- SAFETY IS A HARD REQUIREMENT: treat all input samples/images as untrusted, potentially LIVE malware. Never execute/detonate a sample outside an explicit isolated sandbox; operate on copies; compute and record SHA-256 of every input for evidence integrity; do not exfiltrate or beacon.\n- Ship a clear CLI, a README documenting usage AND the safety assumptions, and a SMOKE TEST that exercises the pipeline against a benign fixture or a mock (do NOT require a real malware sample to pass).\n- Run the smoke test and iterate until it passes. Commit.\n\nA different model will adversarially review the harness for forensic soundness and operational safety. Output a one-paragraph summary.")
    }
    fn fix_prompt(&self, _task: &str, _bl: &str, _cl: &str, round: usize) -> String {
        format!("The reviewer's findings are in .duet/findings-{round}.json. Fix each properly — ESPECIALLY any operational-safety or evidence-integrity blocker (a harness that could detonate untrusted malware or corrupt evidence is unacceptable). For findings you reject, append 'DISMISSED [<file:line>] — <reason>' to .duet/response-{round}.md. Re-run the smoke test, then commit. Output a one-paragraph summary.")
    }
    fn plan_review_prompt(&self, task: &str, bl: &str, _cl: &str) -> String {
        format!("You are the adversarial CRITIC. The engineer ({bl}) wrote a plan in .duet/plan.md to build a forensics/RE harness for:\n{task}\n\nRed-team it: wrong or missing tools, format-handling gaps (will it actually parse the memory image / AVML capture / sample format correctly?), unsound analysis methodology, and above all OPERATIONAL SAFETY holes — could it execute live malware, beacon out, or destroy/mutate evidence? Output a concise prioritized markdown critique.")
    }
    fn review_prompt(&self, task: &str, _bl: &str, _cl: &str, round: usize) -> String {
        format!("You are an adversarial FORENSICS/SECURITY REVIEWER. The engineer's harness is in .duet/round-{round}.diff (read it, and inspect the repo as needed). Task:\n{task}\n\nAssess: (a) FORENSIC SOUNDNESS — correct use of established tools, correct handling of the artifact/memory/sample format, evidence integrity (hashing, working on copies, never mutating originals); (b) OPERATIONAL SAFETY — are untrusted samples ever executed outside a sandbox? any network egress? could it detonate live malware? (c) CORRECTNESS & COMPLETENESS for the task; (d) does the smoke test actually exercise the pipeline without needing a live sample? Severity: an unsafe-execution or evidence-destroying flaw is a blocker; an incorrect or missing analysis capability is a major.\n\nOutput ONLY a JSON object matching this schema (no prose, no fences):\n{REVIEW_SCHEMA}\nUse file:line of the issue. verdict is \"request_changes\" if any blocker or major exists, else \"approve\".")
    }
    fn critic_tools(&self) -> &'static str {
        "WebSearch WebFetch"
    }
    fn local_review_framing(&self, unit_text: &str) -> (String, String) {
        (
            "You are an adversarial forensics/security reviewer focused on forensic soundness and OPERATIONAL SAFETY: untrusted samples must never be executed outside a sandbox, evidence integrity must be preserved (hashing, copies), and the analysis must correctly handle the artifact format.".into(),
            format!("Review the forensics-harness diff below. Flag: executing/detonating untrusted samples, network egress, or mutating original evidence (blocker); incorrect or missing forensic analysis / format handling (major); missing input hashing or safety docs (minor). Output ONLY JSON matching this schema (no prose, no fences):\n{REVIEW_SCHEMA}\nUse file:line.\n\n--- DIFF ---\n{unit_text}"),
        )
    }
    fn capture_unit(&self, repo: &Path, base: &str, round: usize, duet: &Path) -> Result<PathBuf> {
        let diff = duet.join(format!("round-{round}.diff"));
        git::capture_diff(repo, base, &diff)?;
        Ok(diff)
    }
    fn verify(&self, repo: &Path, duet: &Path) -> Convergence {
        if self.no_test {
            return Convergence::Skipped; // user opted out with --no-test
        }
        // Objective gate: the harness's own smoke test must pass. Re-detect in case
        // the build just added one (research/security start with no test command).
        let cmd = if self.test_cmd.is_empty() {
            git::detect_test_cmd(repo).unwrap_or_default()
        } else {
            self.test_cmd.clone()
        };
        if cmd.is_empty() {
            return Convergence::Skipped;
        }
        match git::run_test(repo, &cmd, &duet.join("transcript.log")) {
            Ok(true) => Convergence::Passed,
            Ok(false) => Convergence::Failed(format!("harness smoke test failed: {cmd}")),
            Err(e) => Convergence::Failed(format!("smoke test error: {e}")),
        }
    }
}
