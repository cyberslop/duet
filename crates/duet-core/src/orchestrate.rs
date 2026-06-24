//! The duet engine. Two orchestration modes share the same backends, reporter,
//! domains, and objective gate:
//!
//! * [`execute`] — the adversarial builder ⇄ critic loop (plan → red-team → build
//!   → review⇄fix → final re-review → optional fresh-eyes swap → verify → summary).
//! * [`execute_conductor`] — conductor mode: a long-horizon strategist directs a
//!   tactical implementer (strategize → implement⇄re-direct → critic gate → verify),
//!   for tasks too long-horizon for a single build pass.
//!
//! All progress flows through a [`Reporter`], so both modes power the headless
//! console view and the live TUI identically.

use crate::agent::{run_stream, Agent, Bins, Critic, Ctx, ReviewReq, Role};
use crate::domain::{domain_for, Convergence, Domain};
use crate::events::{claude_final_result, extract_json};
use crate::render::Model;
use crate::report::{FindingRow, Reporter, Sys};
use crate::{git, prompts};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub struct Config {
    pub repo: PathBuf,
    pub task: String,
    pub builder: Model,
    pub critic: Model,
    pub rounds: usize,
    pub do_plan: bool,
    pub plan_only: bool,
    pub review_only: bool,
    pub swap: bool,
    /// `None` → autodetect; `Some("")` → disabled; `Some(cmd)` → use cmd.
    pub test_cmd: Option<String>,
    pub claude_model: Option<String>,
    pub codex_model: Option<String>,
    pub codex_build_sandbox: String,
    pub branch: Option<String>,
    pub base_ref: Option<String>,
    /// When the critic is `local`: endpoint / model id (else env / autodetect).
    pub local_endpoint: Option<String>,
    pub local_model: Option<String>,
    /// Workflow domain: "code" (default) | "research".
    pub domain: String,
    /// Conductor mode: a long-horizon strategist directs a tactical implementer.
    /// When true, `builder` is the IMPLEMENTER and `strategist` is the director.
    pub conductor: bool,
    /// The strategist model in conductor mode (must differ from the implementer;
    /// neither may be `local`, which has no file/exec tools).
    pub strategist: Option<Model>,
}

#[derive(Deserialize, Default)]
struct Findings {
    #[serde(default)]
    verdict: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    findings: Vec<Finding>,
}

#[derive(Deserialize)]
struct Finding {
    #[serde(default)]
    severity: String,
    #[serde(default)]
    file: String,
    #[serde(default)]
    line: i64,
    #[serde(default)]
    issue: String,
}

/// A review verdict.
struct Verdict {
    approve: bool,
    blockers: i64,
    verdict: String,
}

/// The strategist's structured turn in conductor mode: an updated long-horizon
/// plan, the next narrow objective for the implementer, and whether the goal is met.
#[derive(Deserialize, Default)]
struct Strategy {
    #[serde(default)]
    status: String,
    #[serde(default)]
    plan: String,
    #[serde(default)]
    objective: String,
    #[serde(default)]
    progress: String,
}

impl Strategy {
    fn is_done(&self) -> bool {
        self.status.trim().eq_ignore_ascii_case("done")
    }
}

/// Parse a strategist turn from its (possibly fenced or prose-wrapped) output.
/// A reply that isn't valid JSON degrades to "continue" with the raw text as the
/// objective, so a chatty strategist never aborts the run.
fn parse_strategy(raw: &str) -> Strategy {
    serde_json::from_str::<Strategy>(&extract_json(raw)).unwrap_or_else(|_| Strategy {
        status: "continue".into(),
        objective: raw.trim().to_string(),
        ..Default::default()
    })
}

struct Engine<'a> {
    cfg: &'a Config,
    rep: &'a dyn Reporter,
    ctx: Ctx<'a>,
    duet: PathBuf,
    base: String,
    domain: Box<dyn Domain>,
    builder: Option<Box<dyn Agent>>,
    critic: Option<Box<dyn Critic>>,
    step: usize,
}

impl Engine<'_> {
    fn builder(&self) -> &dyn Agent {
        self.builder.as_deref().expect("builder present")
    }
    fn critic(&self) -> &dyn Critic {
        self.critic.as_deref().expect("critic present")
    }
    fn bl(&self) -> &'static str {
        self.builder().model().label()
    }
    fn cl(&self) -> &'static str {
        self.critic().model().label()
    }
    fn critic_model(&self) -> Model {
        self.critic().model()
    }
    fn critic_is_cli(&self) -> bool {
        self.critic().can_build()
    }
    /// Fresh-eyes swap — only valid when the critic can build (guarded by caller).
    fn swap_roles(&mut self) {
        let builder = self.builder.take().expect("builder present");
        let critic = self.critic.take().expect("critic present");
        let (new_builder, new_critic) = critic.swapped(builder);
        self.builder = Some(new_builder);
        self.critic = Some(new_critic);
    }

    fn raw(&mut self, kind: &str) -> PathBuf {
        self.step += 1;
        self.duet.join(format!("stream-{:02}-{kind}.jsonl", self.step))
    }

    fn run_builder(&mut self, prompt: &str) -> Result<()> {
        let kind = format!("build-{}", self.bl());
        let raw = self.raw(&kind);
        run_stream(self.builder(), Role::Build, prompt, None, &raw, self.rep, &self.ctx)
    }

    /// Run the critic and guarantee `out` holds its result. The engine pre-computes
    /// both the CLI prompt and the inlined-unit framing; each backend uses what it
    /// can (CLI critics read the unit via tools, the local critic gets it inlined).
    fn run_critic(&mut self, role: Role, prompt: &str, unit: &Path, out: &Path, strip: bool) -> Result<()> {
        let which = if matches!(role, Role::ReviewText) { "redteam" } else { "review" };
        let kind = format!("{which}-{}", self.cl());
        let raw = self.raw(&kind);
        let unit_text = std::fs::read_to_string(unit).unwrap_or_default();
        let (local_system, local_user) = match role {
            Role::ReviewJson => self.domain.local_review_framing(&unit_text),
            _ => self.domain.local_redteam_framing(&unit_text),
        };
        let schema = self.domain.review_schema();
        let req = ReviewReq {
            role,
            cli_prompt: prompt,
            out,
            raw: &raw,
            schema,
            local_system: &local_system,
            local_user: &local_user,
            strip,
        };
        self.critic().review(&req, &self.ctx, self.rep)
    }

    fn plan(&mut self) -> Result<()> {
        self.rep.phase(&format!("Plan — {} drafts, {} red-teams", self.bl(), self.cl()));
        let p = self.domain.plan_prompt(&self.cfg.task, self.bl(), self.cl());
        self.run_builder(&p)?;
        let plan_review = self.duet.join("plan-review.md");
        let plan_md = self.duet.join("plan.md");
        let pr = self.domain.plan_review_prompt(&self.cfg.task, self.bl(), self.cl());
        self.run_critic(Role::ReviewText, &pr, &plan_md, &plan_review, false)?;
        self.rep.sys(Sys::Ok, "plan red-team written to .duet/plan-review.md");
        let pv = self.domain.plan_revise_prompt(self.bl());
        self.run_builder(&pv)?;
        self.rep.sys(Sys::Ok, "plan revised");
        Ok(())
    }

    fn implement(&mut self) -> Result<()> {
        self.rep.phase(&format!("Build — {} produces the {}", self.bl(), self.domain.unit_kind()));
        let p = self.domain.build_prompt(&self.cfg.task, self.bl());
        self.run_builder(&p)?;
        self.rep.sys(Sys::Ok, "build pass complete");
        Ok(())
    }

    fn verify(&self) -> Convergence {
        self.domain.verify(&self.cfg.repo, &self.duet)
    }

    /// One review: capture the unit, run the critic, parse the verdict.
    fn review(&mut self, round: usize, label: &str) -> Result<Verdict> {
        self.rep.phase(label);
        let unit = self.domain.capture_unit(&self.cfg.repo, &self.base, round, &self.duet)?;
        if std::fs::metadata(&unit).map(|m| m.len() == 0).unwrap_or(true) {
            self.rep.sys(Sys::Warn, &format!("no {} to review", self.domain.unit_kind()));
            return Ok(Verdict { approve: true, blockers: 0, verdict: "approve".into() });
        }
        let findings_path = self.duet.join(format!("findings-{round}.json"));
        let prompt = self.domain.review_prompt(&self.cfg.task, self.bl(), self.cl(), round);
        self.run_critic(Role::ReviewJson, &prompt, &unit, &findings_path, true)?;
        Ok(parse_and_report(self.rep, self.critic_model(), &findings_path))
    }

    fn fix(&mut self, round: usize) -> Result<()> {
        self.rep.phase(&format!("Fix round {round} — {} addresses findings", self.bl()));
        let prompt = self.domain.fix_prompt(&self.cfg.task, self.bl(), self.cl(), round);
        self.run_builder(&prompt)
    }
}

pub fn execute(cfg: &Config, builder: Box<dyn Agent>, critic: Box<dyn Critic>, rep: &dyn Reporter) -> Result<i32> {
    if cfg.builder == cfg.critic {
        return Err(anyhow!("builder and critic must be different models"));
    }
    if cfg.builder == Model::Local {
        return Err(anyhow!("a local chat model can't be the builder (no file/exec tools)"));
    }
    if cfg.task.trim().is_empty() {
        return Err(anyhow!("no task given"));
    }
    let bins = Bins::resolve()?;
    if !git::is_repo(&cfg.repo) {
        return Err(anyhow!("{} is not a git repository (run 'git init' there)", cfg.repo.display()));
    }

    let duet = cfg.repo.join(".duet");
    std::fs::create_dir_all(&duet)?;
    let log = duet.join("transcript.log");
    std::fs::write(&log, b"")?;

    // Some("") means the user explicitly disabled the gate (--no-test); distinct
    // from None (unspecified → autodetect).
    let no_test = matches!(&cfg.test_cmd, Some(s) if s.is_empty());
    let test_cmd = match &cfg.test_cmd {
        Some(s) => s.clone(),
        // research has no test gate; code/security autodetect a smoke test
        None if cfg.domain == "research" => String::new(),
        None => git::detect_test_cmd(&cfg.repo).unwrap_or_default(),
    };
    let domain = domain_for(&cfg.domain, test_cmd.clone(), no_test);
    let schema = duet.join("review.schema.json");
    std::fs::write(&schema, domain.review_schema())?;
    git::ensure_ignored(&cfg.repo);

    let (bl0, cl0) = (cfg.builder.label(), cfg.critic.label());
    rep.phase(&format!("Duet [{}]: {bl0} (builder) ⇄ {cl0} (critic)", domain.name()));
    rep.sys(Sys::Note, &format!("repo:   {}", cfg.repo.display()));
    rep.sys(Sys::Note, &format!("task:   {}", cfg.task.lines().next().unwrap_or("")));
    rep.sys(Sys::Note, &format!("rounds: up to {}   plan: {}   review-only: {}   swap: {}", cfg.rounds, cfg.do_plan, cfg.review_only, cfg.swap));
    rep.sys(Sys::Note, &format!("verify: {}", if test_cmd.is_empty() { domain.name() } else { &test_cmd }));

    if !git::has_head(&cfg.repo) {
        rep.sys(Sys::Warn, "repo has no commits; creating an empty baseline commit");
        git::git_ok(&cfg.repo, &["commit", "--allow-empty", "-m", "duet: baseline"])?;
    }
    let base = git::rev_parse(&cfg.repo, cfg.base_ref.as_deref().unwrap_or("HEAD"))?;
    if !cfg.review_only {
        let branch = cfg.branch.clone().unwrap_or_else(|| format!("duet/{}", epoch()));
        git::checkout_branch(&cfg.repo, &branch)?;
        rep.sys(Sys::Ok, &format!("working on branch: {branch} (base {})", short(&base)));
    }

    let mut eng = Engine {
        cfg,
        rep,
        ctx: Ctx {
            bins: &bins,
            repo: &cfg.repo,
            claude_model: cfg.claude_model.as_deref(),
            codex_model: cfg.codex_model.as_deref(),
            codex_build_sandbox: &cfg.codex_build_sandbox,
            schema: &schema,
            log: &log,
            critic_tools: domain.critic_tools(),
        },
        duet: duet.clone(),
        base: base.clone(),
        domain,
        builder: Some(builder),
        critic: Some(critic),
        step: 0,
    };

    if cfg.do_plan {
        eng.plan()?;
        if cfg.plan_only {
            rep.sys(Sys::Ok, "plan-only mode complete — see .duet/plan.md and .duet/plan-review.md");
            return Ok(0);
        }
    }
    if !cfg.review_only {
        eng.implement()?;
    }

    // ── review ⇄ fix loop ─────────────────────────────────────────────────
    let mut v = Verdict { approve: false, blockers: 99, verdict: "request_changes".into() };
    let mut round = 0usize;
    while round < cfg.rounds {
        round += 1;
        let label = format!("Review round {round}/{} — {} critiques {}'s {}", cfg.rounds, eng.cl(), eng.bl(), eng.domain.unit_kind());
        v = eng.review(round, &label)?;
        if v.approve {
            rep.sys(Sys::Ok, &format!("converged: {} approves after round {round}", eng.cl()));
            break;
        }
        eng.fix(round)?;
    }

    // ── final confirmation review (so we never exit on a dangling fix) ─────
    if !v.approve {
        round += 1;
        v = eng.review(round, "Final review — re-checking the last fix")?;
        if v.approve {
            rep.sys(Sys::Ok, "converged on the final re-review");
        }
    }

    // ── optional fresh-eyes swap (a different model re-reviews) ────────────
    if cfg.swap && v.approve && !eng.critic_is_cli() {
        rep.sys(Sys::Note, "swap skipped — the local critic can't take the builder role");
    } else if cfg.swap && v.approve {
        eng.swap_roles();
        round += 1;
        let label = format!("Fresh eyes — {} re-reviews the final {}", eng.cl(), eng.domain.unit_kind());
        let fresh = eng.review(round, &label)?;
        if fresh.approve {
            rep.sys(Sys::Ok, "fresh-eyes review approves");
            v = fresh; // the swapped critic had the last word — report ITS verdict
        } else {
            rep.sys(Sys::Warn, "fresh-eyes review found issues; addressing once and re-checking");
            eng.fix(round)?;
            round += 1;
            v = eng.review(round, "Fresh eyes — confirm")?;
        }
    }

    // ── independent verification (the domain's objective gate) ────────────
    rep.phase("Verify — orchestrator runs the objective gate independently");
    let (test_status, gate_failed) = match eng.verify() {
        Convergence::Passed => {
            rep.sys(Sys::Ok, "verification passed");
            ("passed", false)
        }
        Convergence::Failed(why) => {
            rep.sys(Sys::Warn, &format!("verification FAILED: {why}"));
            ("FAILED", true)
        }
        Convergence::Skipped => {
            rep.sys(Sys::Note, "no objective gate for this run (ungated)");
            ("skipped", false)
        }
    };

    write_summary(cfg, &duet, &base, bl0, cl0, round, &v.verdict, v.blockers, test_status)?;

    let converged = v.approve && !gate_failed;
    if converged {
        rep.status("approve");
        rep.sys(Sys::Ok, &format!("DUET CONVERGED ✓   summary: {}", duet.join("SUMMARY.md").display()));
        Ok(0)
    } else {
        rep.status(&v.verdict);
        rep.sys(Sys::Warn, &format!("DID NOT FULLY CONVERGE — verdict={} open-blockers={} tests={test_status}", v.verdict, v.blockers));
        rep.sys(Sys::Note, "re-run with `duet review` to continue, or adjudicate in .duet/SUMMARY.md");
        Ok(2)
    }
}

fn parse_findings(p: &Path) -> Option<Findings> {
    let s = std::fs::read_to_string(p).ok()?;
    serde_json::from_str::<Findings>(&s).ok()
}

/// Parse a findings file, render its verdict + findings to the reporter, and
/// return the structured [`Verdict`]. Shared by the duet and conductor critics.
fn parse_and_report(rep: &dyn Reporter, critic_model: Model, findings_path: &Path) -> Verdict {
    match parse_findings(findings_path) {
        Some(f) => {
            // Normalize case: a critic (especially a local model, whose JSON
            // isn't schema-guaranteed) may emit "Approve" or "BLOCKER".
            let verdict = if f.verdict.is_empty() { "request_changes".into() } else { f.verdict.to_ascii_lowercase() };
            let blockers = f
                .findings
                .iter()
                .filter(|x| matches!(x.severity.to_ascii_lowercase().as_str(), "blocker" | "major"))
                .count() as i64;
            rep.say(critic_model, &format!("verdict={verdict}  findings={}  (blocker+major={blockers})", f.findings.len()));
            let rows: Vec<FindingRow> = f
                .findings
                .iter()
                .map(|fd| FindingRow { sev: fd.severity.clone(), file: fd.file.clone(), line: fd.line, issue: fd.issue.clone() })
                .collect();
            rep.findings(&rows);
            rep.status(&verdict);
            Verdict { approve: verdict == "approve" && blockers == 0, blockers, verdict }
        }
        None => {
            rep.sys(Sys::Warn, "critic did not return valid JSON; treating as request_changes");
            Verdict { approve: false, blockers: 1, verdict: "request_changes".into() }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn write_summary(
    cfg: &Config,
    duet: &Path,
    base: &str,
    bl: &str,
    cl: &str,
    round: usize,
    verdict: &str,
    blockers: i64,
    test_status: &str,
) -> Result<()> {
    let mut s = String::new();
    s.push_str("# Duet summary\n\n");
    s.push_str(&format!("- **Task:** {}\n", cfg.task.lines().next().unwrap_or("")));
    s.push_str(&format!("- **Builder / Critic:** {bl} / {cl}\n"));
    s.push_str(&format!("- **Base commit:** {base}\n"));
    s.push_str(&format!("- **Rounds run:** {round} (max {})\n", cfg.rounds));
    s.push_str(&format!("- **Final verdict:** {verdict} — open blocker/major: {blockers}\n"));
    s.push_str(&format!("- **Test gate:** {test_status}\n\n"));
    s.push_str("## Findings by round\n");
    for r in 1..=round {
        if let Some(f) = parse_findings(&duet.join(format!("findings-{r}.json"))) {
            s.push_str(&format!("### round {r}\n{}\n", f.summary));
            for fd in &f.findings {
                s.push_str(&format!("- [{}] {}:{} — {}\n", fd.severity, fd.file, fd.line, fd.issue));
            }
        }
    }
    if cfg.domain == "code" {
        s.push_str("\n## Diffstat since base\n```\n");
        s.push_str(&git::diffstat(&cfg.repo, base));
        s.push_str("```\n");
    }
    std::fs::write(duet.join("SUMMARY.md"), s)?;
    Ok(())
}

fn epoch() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0).to_string()
}

fn short(s: &str) -> String {
    s.chars().take(12).collect()
}

// ───────────────────────────── conductor mode ───────────────────────────────
// A long-horizon STRATEGIST (thorough, holds the goal, never loses the thread)
// directs a tactical IMPLEMENTER (resourceful, but stops early) over many small,
// pointed handoffs — the "strategist + implementer = a frontier model at home"
// ensemble. The strategist reads and decides; the implementer writes. An optional
// adversarial CRITIC gates the whole result, and the domain's objective gate has
// the final word — no model self-grades convergence.

struct ConductorEngine<'a> {
    cfg: &'a Config,
    rep: &'a dyn Reporter,
    ctx: Ctx<'a>,
    duet: PathBuf,
    base: String,
    domain: Box<dyn Domain>,
    strategist: Box<dyn Agent>,
    implementer: Box<dyn Agent>,
    critic: Box<dyn Critic>,
    test_cmd: String,
    step: usize,
}

impl ConductorEngine<'_> {
    fn sl(&self) -> &'static str {
        self.strategist.model().label()
    }
    fn il(&self) -> &'static str {
        self.implementer.model().label()
    }
    fn cl(&self) -> &'static str {
        self.critic.model().label()
    }

    fn raw(&mut self, kind: &str) -> PathBuf {
        self.step += 1;
        self.duet.join(format!("stream-{:02}-{kind}.jsonl", self.step))
    }

    /// Run the strategist (read-only) and capture its final message to `out`,
    /// exactly as the CLI critic does — Codex writes via `-o`, Claude's result is
    /// recovered from its captured stream.
    fn run_strategist(&mut self, prompt: &str, out: &Path) -> Result<()> {
        let kind = format!("strategy-{}", self.sl());
        let raw = self.raw(&kind);
        let model = self.strategist.model();
        let out_arg = matches!(model, Model::Codex).then_some(out);
        run_stream(&*self.strategist, Role::Strategize, prompt, out_arg, &raw, self.rep, &self.ctx)?;
        if matches!(model, Model::Claude) {
            let stream = std::fs::read_to_string(&raw).unwrap_or_default();
            std::fs::write(out, claude_final_result(&stream).unwrap_or_default())?;
        }
        Ok(())
    }

    /// One strategist turn: run it, parse its JSON control message, report the
    /// progress note. A non-JSON reply degrades to "continue" with the raw text as
    /// the objective rather than aborting the run.
    fn strategize(&mut self, prompt: &str, round: usize) -> Result<Strategy> {
        let out = self.duet.join(format!("strategy-{round}.json"));
        self.run_strategist(prompt, &out)?;
        let raw = std::fs::read_to_string(&out).unwrap_or_default();
        let strat = parse_strategy(&raw);
        if !strat.progress.trim().is_empty() {
            self.rep.say(self.strategist.model(), &format!("progress: {}", strat.progress));
        }
        if !strat.is_done() && !strat.objective.trim().is_empty() {
            self.rep.say(self.strategist.model(), &format!("next objective → {}", strat.objective));
        }
        Ok(strat)
    }

    /// Persist the strategist's running long-horizon plan for the progress step
    /// (and for the human) to read.
    fn write_plan(&self, strat: &Strategy) {
        if !strat.plan.trim().is_empty() {
            let _ = std::fs::write(self.duet.join("plan.md"), &strat.plan);
        }
    }

    /// The implementer executes a single scoped objective and commits.
    fn implement(&mut self, objective: &str, round: usize) -> Result<()> {
        self.rep.phase(&format!("Implement round {round} — {} executes the scoped objective", self.il()));
        let _ = std::fs::write(self.duet.join(format!("handoff-{round}.md")), objective);
        let test = if self.test_cmd.is_empty() { "(none configured — verify your change manually)" } else { &self.test_cmd };
        let prompt = prompts::conductor_handoff(objective, test, round);
        let kind = format!("build-{}", self.il());
        let raw = self.raw(&kind);
        run_stream(&*self.implementer, Role::Build, &prompt, None, &raw, self.rep, &self.ctx)
    }

    /// The adversarial critic reviews the cumulative diff (reusing the domain's
    /// review framing and the shared verdict parser).
    fn critic_review(&mut self, round: usize, label: &str) -> Result<Verdict> {
        self.rep.phase(label);
        let unit = self.domain.capture_unit(&self.cfg.repo, &self.base, round, &self.duet)?;
        if std::fs::metadata(&unit).map(|m| m.len() == 0).unwrap_or(true) {
            self.rep.sys(Sys::Warn, &format!("no {} to review", self.domain.unit_kind()));
            return Ok(Verdict { approve: true, blockers: 0, verdict: "approve".into() });
        }
        let findings_path = self.duet.join(format!("findings-{round}.json"));
        let prompt = self.domain.review_prompt(&self.cfg.task, self.il(), self.cl(), round);
        let kind = format!("review-{}", self.cl());
        let raw = self.raw(&kind);
        let unit_text = std::fs::read_to_string(&unit).unwrap_or_default();
        let (local_system, local_user) = self.domain.local_review_framing(&unit_text);
        let schema = self.domain.review_schema();
        let req = ReviewReq {
            role: Role::ReviewJson,
            cli_prompt: &prompt,
            out: &findings_path,
            raw: &raw,
            schema,
            local_system: &local_system,
            local_user: &local_user,
            strip: true,
        };
        self.critic.review(&req, &self.ctx, self.rep)?;
        Ok(parse_and_report(self.rep, self.critic.model(), &findings_path))
    }

    /// Capture the cumulative diff since base so the strategist judges the whole
    /// of the work so far, not just the last handoff.
    fn capture_cumulative(&self) -> Result<()> {
        git::capture_diff(&self.cfg.repo, &self.base, &self.duet.join("cumulative.diff"))
    }
}

pub fn execute_conductor(
    cfg: &Config,
    strategist: Box<dyn Agent>,
    implementer: Box<dyn Agent>,
    critic: Box<dyn Critic>,
    rep: &dyn Reporter,
) -> Result<i32> {
    let strat_model = cfg.strategist.ok_or_else(|| anyhow!("conductor mode needs a strategist model"))?;
    if strat_model == cfg.builder {
        return Err(anyhow!("strategist and implementer must be different models"));
    }
    if strat_model == Model::Local || cfg.builder == Model::Local {
        return Err(anyhow!("the strategist and implementer need file/exec tools — neither can be a local chat model"));
    }
    if cfg.task.trim().is_empty() {
        return Err(anyhow!("no task given"));
    }
    let bins = Bins::resolve()?;
    if !git::is_repo(&cfg.repo) {
        return Err(anyhow!("{} is not a git repository (run 'git init' there)", cfg.repo.display()));
    }

    let duet = cfg.repo.join(".duet");
    std::fs::create_dir_all(&duet)?;
    let log = duet.join("transcript.log");
    std::fs::write(&log, b"")?;

    let no_test = matches!(&cfg.test_cmd, Some(s) if s.is_empty());
    let test_cmd = match &cfg.test_cmd {
        Some(s) => s.clone(),
        None if cfg.domain == "research" => String::new(),
        None => git::detect_test_cmd(&cfg.repo).unwrap_or_default(),
    };
    let domain = domain_for(&cfg.domain, test_cmd.clone(), no_test);
    let schema = duet.join("review.schema.json");
    std::fs::write(&schema, domain.review_schema())?;
    git::ensure_ignored(&cfg.repo);

    rep.phase(&format!(
        "Conductor [{}]: {} (strategist) → {} (implementer) · {} (critic)",
        domain.name(),
        strat_model.label(),
        cfg.builder.label(),
        cfg.critic.label()
    ));
    rep.sys(Sys::Note, &format!("repo:   {}", cfg.repo.display()));
    rep.sys(Sys::Note, &format!("task:   {}", cfg.task.lines().next().unwrap_or("")));
    rep.sys(Sys::Note, &format!("max iterations: {}   verify: {}", cfg.rounds, if test_cmd.is_empty() { domain.name() } else { &test_cmd }));

    if !git::has_head(&cfg.repo) {
        rep.sys(Sys::Warn, "repo has no commits; creating an empty baseline commit");
        git::git_ok(&cfg.repo, &["commit", "--allow-empty", "-m", "duet: baseline"])?;
    }
    let base = git::rev_parse(&cfg.repo, cfg.base_ref.as_deref().unwrap_or("HEAD"))?;
    let branch = cfg.branch.clone().unwrap_or_else(|| format!("duet/{}", epoch()));
    git::checkout_branch(&cfg.repo, &branch)?;
    rep.sys(Sys::Ok, &format!("working on branch: {branch} (base {})", short(&base)));

    let mut eng = ConductorEngine {
        cfg,
        rep,
        ctx: Ctx {
            bins: &bins,
            repo: &cfg.repo,
            claude_model: cfg.claude_model.as_deref(),
            codex_model: cfg.codex_model.as_deref(),
            codex_build_sandbox: &cfg.codex_build_sandbox,
            schema: &schema,
            log: &log,
            critic_tools: domain.critic_tools(),
        },
        duet: duet.clone(),
        base: base.clone(),
        domain,
        strategist,
        implementer,
        critic,
        test_cmd: test_cmd.clone(),
        step: 0,
    };

    // ── strategist sets the long-horizon plan and the first scoped objective ──
    eng.rep.phase(&format!("Strategy — {} sets the long-horizon plan and first objective", eng.sl()));
    let p0 = prompts::conductor_strategy(&cfg.task, eng.sl(), eng.il());
    let mut strat = eng.strategize(&p0, 0)?;
    eng.write_plan(&strat);
    if strat.objective.trim().is_empty() && !strat.is_done() {
        rep.sys(Sys::Warn, "strategist produced no first objective; handing the implementer the whole task");
        strat.objective = cfg.task.clone();
    }

    // ── implement ⇄ re-direct loop, strategist-led ──
    let mut round = 0usize;
    let mut done = strat.is_done();
    while round < cfg.rounds && !done {
        round += 1;
        let objective = strat.objective.clone();
        eng.implement(&objective, round)?;
        eng.capture_cumulative()?;
        eng.rep.phase(&format!("Direct round {round}/{} — {} judges progress and re-points", cfg.rounds, eng.sl()));
        let pp = prompts::conductor_progress(&cfg.task, round);
        strat = eng.strategize(&pp, round)?;
        eng.write_plan(&strat);
        done = strat.is_done();
        if done {
            rep.sys(Sys::Ok, &format!("strategist judges the long-horizon goal met after {round} iteration(s)"));
        } else if strat.objective.trim().is_empty() {
            rep.sys(Sys::Warn, "strategist gave no next objective and did not declare done — ending the loop");
            break;
        }
    }
    if !done && round >= cfg.rounds {
        rep.sys(Sys::Note, &format!("reached the iteration cap ({}) before the strategist declared the goal met", cfg.rounds));
    }

    // ── adversarial critic gate over the cumulative result ──
    let mut v = eng.critic_review(round + 1, &format!("Critic gate — {} reviews the whole result", eng.cl()))?;
    if v.approve {
        rep.sys(Sys::Ok, &format!("critic approves after round {}", round + 1));
    } else {
        // One strategist-directed corrective pass, then a confirming re-review.
        round += 1;
        rep.phase(&format!("Correct — {} addresses the critic's blockers", eng.il()));
        let fix = eng.domain.fix_prompt(&cfg.task, eng.il(), eng.cl(), round);
        let kind = format!("build-{}", eng.il());
        let raw = eng.raw(&kind);
        run_stream(&*eng.implementer, Role::Build, &fix, None, &raw, rep, &eng.ctx)?;
        v = eng.critic_review(round + 1, "Critic gate — re-checking the correction")?;
    }

    // ── independent objective gate (mechanical; never LLM self-grading) ──
    rep.phase("Verify — orchestrator runs the objective gate independently");
    let (test_status, gate_failed) = match eng.domain.verify(&cfg.repo, &duet) {
        Convergence::Passed => {
            rep.sys(Sys::Ok, "verification passed");
            ("passed", false)
        }
        Convergence::Failed(why) => {
            rep.sys(Sys::Warn, &format!("verification FAILED: {why}"));
            ("FAILED", true)
        }
        Convergence::Skipped => {
            rep.sys(Sys::Note, "no objective gate for this run (ungated)");
            ("skipped", false)
        }
    };

    let bl = format!("{}\u{2192}{}", strat_model.label(), cfg.builder.label());
    write_summary(cfg, &duet, &base, &bl, cfg.critic.label(), round + 1, &v.verdict, v.blockers, test_status)?;

    let converged = v.approve && !gate_failed && (done || round >= cfg.rounds);
    if converged && done {
        rep.status("approve");
        rep.sys(Sys::Ok, &format!("CONDUCTOR CONVERGED ✓   summary: {}", duet.join("SUMMARY.md").display()));
        Ok(0)
    } else {
        rep.status(&v.verdict);
        let why = if !done { "strategist did not declare the goal met" } else { "critic or objective gate not green" };
        rep.sys(Sys::Warn, &format!("DID NOT FULLY CONVERGE ({why}) — verdict={} open-blockers={} tests={test_status}", v.verdict, v.blockers));
        Ok(2)
    }
}

#[cfg(test)]
mod conductor_tests {
    use super::{parse_strategy, Strategy};

    #[test]
    fn parses_clean_json() {
        let s = parse_strategy(r#"{"status":"continue","plan":"do X then Y","objective":"edit src/foo.rs add a guard","progress":"nothing yet"}"#);
        assert!(!s.is_done());
        assert_eq!(s.objective, "edit src/foo.rs add a guard");
        assert_eq!(s.plan, "do X then Y");
    }

    #[test]
    fn done_is_case_insensitive_and_status_aware() {
        assert!(parse_strategy(r#"{"status":"DONE","objective":""}"#).is_done());
        assert!(parse_strategy(r#"{"status":"done"}"#).is_done());
        assert!(!parse_strategy(r#"{"status":"continue"}"#).is_done());
    }

    #[test]
    fn unwraps_fenced_json() {
        // extract_json should peel a ```json fence the strategist wrapped around it.
        let s = parse_strategy("here you go:\n```json\n{\"status\":\"continue\",\"objective\":\"step one\"}\n```\n");
        assert_eq!(s.objective, "step one");
        assert!(!s.is_done());
    }

    #[test]
    fn non_json_degrades_to_continue_with_raw_objective() {
        // A chatty, non-JSON reply must not abort the loop: treat it as the next step.
        let s = parse_strategy("Refactor the parser in events.rs to handle empty lines.");
        assert!(!s.is_done());
        assert_eq!(s.objective, "Refactor the parser in events.rs to handle empty lines.");
    }

    #[test]
    fn empty_strategy_is_not_done() {
        let s = Strategy::default();
        assert!(!s.is_done());
        assert!(s.objective.is_empty());
    }
}
