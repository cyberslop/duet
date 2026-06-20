//! The duet engine: an explicit phase/round state machine driving the two agents
//! (plan → red-team → build → review⇄fix → final re-review → optional fresh-eyes
//! swap → verify → summary). All progress flows through a [`Reporter`], so the
//! same engine powers the headless console view and the live TUI.

use crate::agent::{run_stream, Agent, Bins, Critic, Ctx, ReviewReq, Role};
use crate::domain::{domain_for, Convergence, Domain};
use crate::render::Model;
use crate::report::{FindingRow, Reporter, Sys};
use crate::git;
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

        match parse_findings(&findings_path) {
            Some(f) => {
                // Normalize case: a critic (especially a local model, whose JSON
                // isn't schema-guaranteed) may emit "Approve" or "BLOCKER".
                let verdict = if f.verdict.is_empty() { "request_changes".into() } else { f.verdict.to_ascii_lowercase() };
                let blockers = f
                    .findings
                    .iter()
                    .filter(|x| matches!(x.severity.to_ascii_lowercase().as_str(), "blocker" | "major"))
                    .count() as i64;
                self.rep.say(self.critic_model(), &format!("verdict={verdict}  findings={}  (blocker+major={blockers})", f.findings.len()));
                let rows: Vec<FindingRow> = f
                    .findings
                    .iter()
                    .map(|fd| FindingRow { sev: fd.severity.clone(), file: fd.file.clone(), line: fd.line, issue: fd.issue.clone() })
                    .collect();
                self.rep.findings(&rows);
                self.rep.status(&verdict);
                Ok(Verdict { approve: verdict == "approve" && blockers == 0, blockers, verdict })
            }
            None => {
                self.rep.sys(Sys::Warn, "critic did not return valid JSON; treating as request_changes");
                Ok(Verdict { approve: false, blockers: 1, verdict: "request_changes".into() })
            }
        }
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
