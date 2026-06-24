//! `duet` CLI — thin clap front-end over the library.

use anyhow::{anyhow, Result};
use clap::{Args, Parser, Subcommand};
use duet_agents::{resolve_claude, resolve_codex};
use duet_core::events::{parse_claude_line, parse_codex_line, AgentEvent};
use duet_core::orchestrate::{execute, execute_conductor, Config};
use duet_core::render::{model_of_filename, render_line, render_stream, Model};
use duet_core::report::{ChannelReporter, ConsoleReporter, Sys, UiMsg};
use duet_core::style::Theme;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

mod profile;
mod repl;

#[derive(Parser)]
#[command(name = "duet", version, about = "Claude Code ⇄ OpenAI Codex adversarial development")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Full loop: plan → red-team → build → review⇄fix → verify
    Run(RunArgs),
    /// Review-only: critique & fix the current uncommitted changes
    Review(RunArgs),
    /// Plan + Codex red-team only (no code written)
    Plan(RunArgs),
    /// Rich TUI viewer of a run's conversation (interactive; `--snapshot` renders one frame)
    Tui {
        repo: Option<PathBuf>,
        #[arg(long)]
        snapshot: bool,
        #[arg(long)]
        width: Option<u16>,
        #[arg(long)]
        height: Option<u16>,
    },
    /// Follow a duet running in another terminal/background, rendering live
    Watch { repo: Option<PathBuf> },
    /// Re-render a saved conversation (a .duet dir, default cwd, or a single stream file)
    Replay { path: Option<PathBuf> },
    /// Print the last run's SUMMARY.md
    Show { repo: Option<PathBuf> },
    /// Print the last run's transcript
    Log { repo: Option<PathBuf> },
    /// Check prerequisites (claude, codex, login, git)
    Doctor,
    /// Probe this device and recommend local models for a workflow/domain
    SuggestModels {
        /// Workflow domain: code | research | security
        #[arg(long, default_value = "code")]
        domain: String,
    },
    /// Critique the current git diff with a LOCAL model (LM Studio / LiteLLM)
    LocalReview {
        repo: Option<PathBuf>,
        /// Override the served model id (else first from /models, or $DUET_LOCAL_MODEL)
        #[arg(long)]
        model: Option<String>,
    },
    /// List available profiles (role→model bundles)
    Profiles,
}

#[derive(Args)]
struct RunArgs {
    /// Task description (remaining words are joined)
    task: Vec<String>,
    #[arg(long)]
    repo: Option<PathBuf>,
    #[arg(long, default_value = "claude")]
    builder: String,
    #[arg(long, default_value = "codex")]
    critic: String,
    #[arg(long, default_value_t = 3)]
    rounds: usize,
    #[arg(long)]
    no_plan: bool,
    #[arg(long)]
    test: Option<String>,
    #[arg(long)]
    no_test: bool,
    #[arg(long)]
    base: Option<String>,
    #[arg(long)]
    branch: Option<String>,
    #[arg(long)]
    claude_model: Option<String>,
    #[arg(long)]
    codex_model: Option<String>,
    #[arg(long)]
    codex_danger: bool,
    /// Workflow domain: code (default) | research | security
    #[arg(long, default_value = "code")]
    domain: String,
    /// Use a named profile (see `duet profiles`) for the role→model wiring
    #[arg(long)]
    profile: Option<String>,
    /// After convergence, swap roles for one fresh-eyes review by the other model
    #[arg(long)]
    swap: bool,
    /// Conductor mode: a long-horizon strategist directs a tactical implementer
    /// (--builder is the implementer; pick the director with --strategist)
    #[arg(long)]
    conductor: bool,
    /// In conductor mode, the strategist (director) model: claude | codex
    #[arg(long)]
    strategist: Option<String>,
    #[arg(short = 'y', long)]
    yes: bool,
    /// Drive the run through the live ratatui TUI instead of the streaming view
    #[arg(long)]
    tui: bool,
    /// With --tui: render frames to stdout as the run progresses (headless, capturable)
    #[arg(long)]
    record: bool,
}

fn main() {
    let cli = Cli::parse();
    let th = Theme::detect();
    let code = match run(cli, &th) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}error:{} {e}", th.err, th.rst);
            1
        }
    };
    std::process::exit(code);
}

fn run(cli: Cli, th: &Theme) -> Result<i32> {
    let cmd = match cli.cmd {
        Some(c) => c,
        None => return repl::run_session(th), // bare `duet` → interactive session
    };
    match cmd {
        Cmd::Run(a) => dispatch(a, false, false, th),
        Cmd::Review(a) => dispatch(a, true, false, th),
        Cmd::Plan(a) => dispatch(a, false, true, th),
        Cmd::Tui { repo, snapshot, width, height } => {
            let dir = duet_dir(repo);
            let mut app = duet_tui::App::from_duet(&dir)?;
            if snapshot {
                print!("{}", duet_tui::snapshot(&mut app, width.unwrap_or(110), height.unwrap_or(34)));
            } else {
                duet_tui::run(&mut app)?;
            }
            Ok(0)
        }
        Cmd::Watch { repo } => {
            watch(repo, th)?;
            Ok(0)
        }
        Cmd::Replay { path } => {
            replay(path, th)?;
            Ok(0)
        }
        Cmd::Show { repo } => print_file(duet_dir(repo).join("SUMMARY.md")),
        Cmd::Log { repo } => print_file(duet_dir(repo).join("transcript.log")),
        Cmd::Doctor => {
            doctor(th);
            Ok(0)
        }
        Cmd::SuggestModels { domain } => {
            suggest_models(&domain, th);
            Ok(0)
        }
        Cmd::LocalReview { repo, model } => local_review(repo, model, th),
        Cmd::Profiles => {
            list_profiles(th);
            Ok(0)
        }
    }
}

fn list_profiles(th: &Theme) {
    for p in profile::load_all() {
        let swap = if p.swap { " · swap" } else { "" };
        let plan = if p.no_plan { " · no-plan" } else { "" };
        let dom = if p.domain == "code" { String::new() } else { format!(" · {}", p.domain) };
        let roles = if p.conductor {
            format!("{} → {} · {}", p.strategist.as_deref().unwrap_or("codex"), p.builder, p.critic)
        } else {
            format!("{} ⇄ {}", p.builder, p.critic)
        };
        let unit = if p.conductor { "iters" } else { "rounds" };
        println!(
            "{}{:<22}{} {roles}  ({} {unit}{swap}{plan}{dom})",
            th.bold, p.name, th.rst, p.rounds
        );
        if let Some(d) = &p.description {
            println!("{}  {}{}", th.dim, d, th.rst);
        }
    }
    println!("\n{}use: duet run --profile <name> \"<task>\"   ·   add your own in ~/.config/duet/profiles.toml{}", th.dim, th.rst);
}

/// Capture the current diff and have a local model critique it (structured JSON).
fn local_review(repo: Option<PathBuf>, model: Option<String>, th: &Theme) -> Result<i32> {
    use duet_agents::{default_endpoint, LocalBackend};
    use duet_core::report::ConsoleReporter;
    let schema = duet_core::REVIEW_SCHEMA;

    let repo = repo.unwrap_or(std::env::current_dir()?);
    let endpoint = default_endpoint();

    // resolve a served model id
    let model = match model.or_else(|| std::env::var("DUET_LOCAL_MODEL").ok()) {
        Some(m) => m,
        None => match LocalBackend::list_models(&endpoint, 10) {
            Ok(ms) if !ms.is_empty() => ms[0].clone(),
            Ok(_) => return Err(anyhow!("no model loaded at {endpoint} — load one in LM Studio")),
            Err(e) => return Err(anyhow!("no local server at {endpoint} ({e}) — start LM Studio (Developer → Start Server)")),
        },
    };
    println!("{}local critic{} {} @ {}", th.local, th.rst, model, endpoint);

    // The empty tree object, used to diff a single-commit repo against "nothing".
    const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    // Capture the diff non-destructively. Staging everything is the only way to
    // include untracked files in the diff, but a read-only critique must not
    // mutate the user's index, so we save it first and restore it after.
    let saved = git_out(&repo, &["write-tree"]).trim().to_string();
    let _ = std::process::Command::new("git").arg("-C").arg(&repo).args(["add", "-A"]).output();
    let mut diff = git_out(&repo, &["--no-pager", "diff", "--cached", "HEAD"]);
    if !saved.is_empty() {
        let _ = std::process::Command::new("git").arg("-C").arg(&repo).args(["read-tree", &saved]).output();
    }
    if diff.trim().is_empty() {
        // No working changes: review the most recent commit. Diff against its
        // parent, or the empty tree when there is no parent (single-commit repo).
        let has_parent = !git_out(&repo, &["rev-parse", "--quiet", "--verify", "HEAD~1"]).trim().is_empty();
        let base = if has_parent { "HEAD~1" } else { EMPTY_TREE };
        diff = git_out(&repo, &["--no-pager", "diff", base, "HEAD"]);
    }
    if diff.trim().is_empty() {
        return Err(anyhow!("no changes to review in {}", repo.display()));
    }

    let system = "You are an adversarial code reviewer from a different lab than the author. Find real, line-citable bugs, security holes, missing tests, and unhandled edge cases. Prefer a few high-confidence findings over many weak ones.";
    let user = format!(
        "Review the following diff. Output ONLY a JSON object matching this schema (no prose, no fences):\n{schema}\n\nverdict is \"request_changes\" if any blocker/major exists, else \"approve\".\n\n--- DIFF ---\n{diff}"
    );

    let duet = repo.join(".duet");
    std::fs::create_dir_all(&duet).ok();
    let raw = duet.join("local-review.sse.jsonl");
    let backend = LocalBackend::new(&endpoint, &model, std::env::var("LOCAL_API_KEY").ok(), 600);
    let rep = ConsoleReporter { theme: *th };

    let content = backend.critique(system, &user, Some(schema), &raw, &rep)?;

    // parse + summarize
    let json = duet_core::events::extract_json(&content);
    match serde_json::from_str::<serde_json::Value>(&json) {
        Ok(v) => {
            let verdict = v.get("verdict").and_then(|x| x.as_str()).unwrap_or("?");
            let empty: Vec<serde_json::Value> = Vec::new();
            let findings = v.get("findings").and_then(|x| x.as_array()).unwrap_or(&empty);
            println!("\n{}verdict{} {}  ({} findings)", th.bold, th.rst, verdict, findings.len());
            for f in findings {
                let g = |k: &str| f.get(k).and_then(|x| x.as_str()).unwrap_or("");
                let line = f.get("line").and_then(|x| x.as_i64()).unwrap_or(0);
                println!("  [{}] {}:{} — {}", g("severity"), g("file"), line, g("issue"));
            }
            Ok(if verdict.eq_ignore_ascii_case("approve") { 0 } else { 2 })
        }
        Err(_) => {
            println!("\n{}! local model did not return valid JSON; raw output saved to {}{}", th.warn, raw.display(), th.rst);
            Ok(2)
        }
    }
}

fn git_out(repo: &Path, args: &[&str]) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

fn suggest_models(domain: &str, th: &Theme) {
    use duet_core::advisor::{advise, tasks_for_domain};
    use duet_core::hardware::{detect, Accelerator};
    let hw = detect();
    println!("{}device{}  {}", th.bold, th.rst, hw.summary());
    if hw.accelerator == Accelerator::Cpu {
        println!("{}  no GPU detected — local models will be slow; cloud is usually the better call{}", th.dim, th.rst);
    }
    println!("{}domain{}  {}\n", th.bold, th.rst, domain);

    for task in tasks_for_domain(domain) {
        let a = advise(&hw, task);
        println!("{}▸ {:?}{}", th.bold, task, th.rst);
        match a.capable {
            Some(r) => println!(
                "  {}capable local critic{} → {} {} (~{:.0} GB)\n      {}pull:{} {}   — {}",
                th.ok, th.rst, r.name, r.quant.label(), r.footprint_gb, th.dim, th.rst, r.pull_id, r.note
            ),
            None => println!("  {}no viable local model fits this device → route this role to cloud{}", th.warn, th.rst),
        }
        if let Some(r) = a.fast {
            println!(
                "  {}fast/high-volume triage{} → {} {} (~{:.0} GB)   {}{}{}",
                th.dim, th.rst, r.name, r.quant.label(), r.footprint_gb, th.dim, r.pull_id, th.rst
            );
        }
        println!();
    }
    println!("{}Load one in LM Studio (Developer → search the pull id → Start Server :1234),{}", th.dim, th.rst);
    println!("{}then use it as a critic: duet review --critic local{}", th.dim, th.rst);
}

fn dispatch(a: RunArgs, review_only: bool, plan_only: bool, th: &Theme) -> Result<i32> {
    let (tui, record) = (a.tui, a.record);
    let cfg = to_config(a, review_only, plan_only)?;
    if tui {
        run_with_tui(cfg, record)
    } else {
        run_engine(cfg, th)
    }
}

/// Construct the backends for `cfg` and run the engine with the console reporter.
/// Shared by `dispatch` and the interactive session.
fn run_engine(cfg: Config, th: &Theme) -> Result<i32> {
    let rep = ConsoleReporter { theme: *th };
    let critic = duet_agents::build_critic(cfg.critic, cfg.local_endpoint.as_deref(), cfg.local_model.as_deref(), &rep)?;
    if cfg.conductor {
        let strategist = duet_agents::agent_for(cfg.strategist.expect("conductor mode requires a strategist"));
        let implementer = duet_agents::agent_for(cfg.builder);
        execute_conductor(&cfg, strategist, implementer, critic, &rep)
    } else {
        let builder = duet_agents::agent_for(cfg.builder);
        execute(&cfg, builder, critic, &rep)
    }
}

/// Run the engine on a background thread, feeding the live TUI (or the headless
/// recorder) over a channel.
fn run_with_tui(cfg: Config, record: bool) -> Result<i32> {
    use std::sync::mpsc::channel;
    let title = cfg
        .repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "duet".into());
    let (tx, rx) = channel();
    let handle = std::thread::spawn(move || {
        let rep = ChannelReporter { tx: tx.clone() };
        let code = match duet_agents::build_critic(cfg.critic, cfg.local_endpoint.as_deref(), cfg.local_model.as_deref(), &rep) {
            Ok(critic) => {
                let run = if cfg.conductor {
                    let strategist = duet_agents::agent_for(cfg.strategist.expect("conductor mode requires a strategist"));
                    let implementer = duet_agents::agent_for(cfg.builder);
                    execute_conductor(&cfg, strategist, implementer, critic, &rep)
                } else {
                    let builder = duet_agents::agent_for(cfg.builder);
                    execute(&cfg, builder, critic, &rep)
                };
                match run {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(UiMsg::Sys(Sys::Warn, format!("error: {e}")));
                        2
                    }
                }
            }
            Err(e) => {
                let _ = tx.send(UiMsg::Sys(Sys::Warn, format!("error: {e}")));
                2
            }
        };
        let _ = tx.send(UiMsg::Done(code));
        code
    });
    if record {
        duet_tui::record(rx, title, 116, 30);
    } else {
        duet_tui::run_live(rx, title)?;
    }
    Ok(handle.join().unwrap_or(1))
}

fn to_config(a: RunArgs, review_only: bool, plan_only: bool) -> Result<Config> {
    // A profile supplies the base role/round/test wiring; bare flags still apply
    // for repo/models/branch. (Explicit --builder/--critic without --profile work as before.)
    let prof = a.profile.as_deref().map(profile::find).transpose()?;
    let p = prof.as_ref();

    let builder_s = p.map(|p| p.builder.clone()).unwrap_or(a.builder);
    let critic_s = p.map(|p| p.critic.clone()).unwrap_or(a.critic);
    let rounds = p.map(|p| p.rounds).unwrap_or(a.rounds);
    let swap = p.map(|p| p.swap).unwrap_or(a.swap);
    let no_plan = p.map(|p| p.no_plan).unwrap_or(a.no_plan);
    let (local_endpoint, local_model) = match p {
        Some(p) => (p.local_endpoint.clone(), p.local_model.clone()),
        None => (None, None),
    };
    let domain = match p {
        Some(p) if p.domain != "code" => p.domain.clone(),
        _ => a.domain,
    };
    if !matches!(domain.as_str(), "code" | "research" | "security") {
        return Err(anyhow!("unknown --domain '{domain}' (use code | research | security)"));
    }

    let builder = Model::parse(&builder_s).ok_or_else(|| anyhow!("builder must be claude|codex"))?;
    let critic = Model::parse(&critic_s).ok_or_else(|| anyhow!("critic must be claude|codex|local"))?;
    if builder == Model::Local {
        return Err(anyhow!("a local chat model can't be the builder (no file/exec tools) — use claude|codex"));
    }

    // Conductor mode: a long-horizon strategist directs the implementer (= builder).
    let conductor = p.map(|p| p.conductor).unwrap_or(false) || a.conductor;
    let strategist = if conductor {
        // Profile's strategist wins; else the --strategist flag; else codex as a
        // functional default (the cross-vendor pairing is the point, not the order).
        let s = p.and_then(|p| p.strategist.clone()).or(a.strategist).unwrap_or_else(|| "codex".into());
        let m = Model::parse(&s).ok_or_else(|| anyhow!("strategist must be claude|codex"))?;
        if m == Model::Local {
            return Err(anyhow!("the strategist needs file/exec tools — use claude|codex"));
        }
        if m == builder {
            return Err(anyhow!("strategist and implementer (--builder) must be different models"));
        }
        Some(m)
    } else {
        None
    };
    let repo = a.repo.unwrap_or(std::env::current_dir()?);
    let repo = repo.canonicalize().unwrap_or(repo);
    let test_cmd = if a.no_test {
        Some(String::new())
    } else {
        a.test.or_else(|| p.and_then(|p| p.test_cmd.clone()))
    };
    let do_plan = if review_only {
        false
    } else if plan_only {
        true
    } else {
        !no_plan
    };
    let mut task = a.task.join(" ");
    if task.trim().is_empty() && review_only {
        task = "Review and harden the current uncommitted changes.".into();
    }
    Ok(Config {
        repo,
        task,
        builder,
        critic,
        rounds,
        do_plan,
        plan_only,
        review_only,
        test_cmd,
        claude_model: a.claude_model,
        codex_model: a.codex_model,
        codex_build_sandbox: if a.codex_danger { "danger-full-access".into() } else { "workspace-write".into() },
        branch: a.branch,
        base_ref: a.base,
        swap,
        local_endpoint,
        local_model,
        domain,
        conductor,
        strategist,
    })
}

fn duet_dir(repo: Option<PathBuf>) -> PathBuf {
    repo.unwrap_or_else(|| PathBuf::from(".")).join(".duet")
}

fn print_file(p: PathBuf) -> Result<i32> {
    let s = std::fs::read_to_string(&p).map_err(|_| anyhow!("not found: {} (run a duet first)", p.display()))?;
    print!("{s}");
    Ok(0)
}

fn replay(path: Option<PathBuf>, th: &Theme) -> Result<()> {
    let p = path.unwrap_or_else(|| PathBuf::from("."));
    // a single stream file
    if p.is_file() {
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        let m = model_of_filename(&name).ok_or_else(|| anyhow!("cannot tell which model produced {name}"))?;
        render_stream(th, m, &std::fs::read_to_string(&p)?);
        return Ok(());
    }
    // otherwise a repo or .duet directory
    let dir = if p.ends_with(".duet") { p } else { p.join(".duet") };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|_| anyhow!("no .duet/ found at {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|f| {
            let n = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            n.starts_with("stream-") && n.ends_with(".jsonl")
        })
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(anyhow!("no saved conversation streams in {}", dir.display()));
    }
    for f in files {
        let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
        if let Some(m) = model_of_filename(&name) {
            println!("{}── {name} ──{}", th.dim, th.rst);
            render_stream(th, m, &std::fs::read_to_string(&f)?);
        }
    }
    Ok(())
}

/// Follow a duet running elsewhere: poll the repo's `.duet/` for stream files and
/// render newly-appended events as they're written. Ctrl-C to stop.
fn watch(repo: Option<PathBuf>, th: &Theme) -> Result<()> {
    let dir = duet_dir(repo);
    eprintln!("{}watching {} — Ctrl-C to stop{}", th.dim, dir.display(), th.rst);
    let mut processed: HashMap<PathBuf, usize> = HashMap::new();
    let mut announced: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    loop {
        if dir.is_dir() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|f| {
                    let n = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    n.starts_with("stream-") && n.ends_with(".jsonl")
                })
                .collect();
            files.sort();
            for f in &files {
                let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let Some(model) = model_of_filename(&name) else { continue };
                let parse: fn(&str) -> Vec<AgentEvent> = match model {
                    Model::Claude => parse_claude_line,
                    Model::Codex => parse_codex_line,
                    Model::Local => duet_core::events::parse_local_line,
                };
                let content = std::fs::read_to_string(f).unwrap_or_default();
                let lines: Vec<&str> = content.split('\n').collect();
                // drop the trailing element (empty after a final '\n', or a mid-write partial line)
                let complete = lines.len().saturating_sub(1);
                let done = *processed.get(f).unwrap_or(&0);
                if done >= complete {
                    continue;
                }
                if announced.insert(f.clone()) {
                    println!("{}── {} ──{}", th.dim, watch_label(&name), th.rst);
                }
                for line in &lines[done..complete] {
                    for ev in parse(line) {
                        if let Some(s) = render_line(th, model, &ev) {
                            println!("{s}");
                        }
                    }
                }
                processed.insert(f.clone(), complete);
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn watch_label(filename: &str) -> String {
    let parts: Vec<&str> = filename.trim_end_matches(".jsonl").split('-').collect();
    let pretty = match parts.get(2).copied().unwrap_or("") {
        "plan" => "Plan",
        "redteam" => "Plan red-team",
        "planrev" => "Plan revise",
        "strategy" => "Strategize",
        "build" => "Build",
        "review" => "Review",
        other => other,
    };
    format!("{pretty} · {}", parts.get(3).copied().unwrap_or(""))
}

fn doctor(th: &Theme) {
    let line = |label: &str, v: Result<PathBuf>| match v {
        Ok(p) => println!("{}{label:<8}{} {}", th.ok, th.rst, p.display()),
        Err(e) => println!("{}{label:<8}{} MISSING — {e}", th.err, th.rst),
    };
    println!("git      {}", which("git"));
    line("claude", resolve_claude());
    line("codex", resolve_codex());
    if let Ok(p) = resolve_codex() {
        print!("  codex login: ");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let _ = std::process::Command::new(p).args(["login", "status"]).status();
    }
    // local OpenAI-compatible endpoint (LM Studio / LiteLLM / …)
    let endpoint = duet_agents::default_endpoint();
    match duet_agents::LocalBackend::list_models(&endpoint, 3) {
        Ok(ms) if !ms.is_empty() => println!("{}local{}    {endpoint}  →  {}", th.ok, th.rst, ms.join(", ")),
        Ok(_) => println!("{}local{}    {endpoint}  reachable but no model loaded", th.warn, th.rst),
        Err(_) => println!("{}local{}    {endpoint}  not reachable (start LM Studio: Developer → Start Server)", th.dim, th.rst),
    }
}

fn which(name: &str) -> String {
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|p| p.join(name))
                .find(|p| p.is_file())
                .map(|p| p.display().to_string())
        })
        .unwrap_or_else(|| "MISSING".into())
}
