//! Thin git + test-gate helpers (the orchestration's substrate).

use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::{Command, Stdio};

fn git_raw(repo: &Path, args: &[&str]) -> Result<std::process::Output> {
    Ok(Command::new("git").arg("-C").arg(repo).args(args).output()?)
}

pub fn git_ok(repo: &Path, args: &[&str]) -> Result<String> {
    let out = git_raw(repo, args)?;
    if !out.status.success() {
        return Err(anyhow!("git {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn is_repo(repo: &Path) -> bool {
    git_raw(repo, &["rev-parse", "--is-inside-work-tree"])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn has_head(repo: &Path) -> bool {
    git_raw(repo, &["rev-parse", "--verify", "HEAD"])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn rev_parse(repo: &Path, refname: &str) -> Result<String> {
    git_ok(repo, &["rev-parse", refname])
}

pub fn checkout_branch(repo: &Path, branch: &str) -> Result<()> {
    if git_raw(repo, &["rev-parse", "--verify", branch])?.status.success() {
        git_ok(repo, &["checkout", branch])?;
    } else {
        git_ok(repo, &["checkout", "-b", branch])?;
    }
    Ok(())
}

/// Stage everything and write the full diff since `base` (committed + staged +
/// unstaged + new files) to `out`.
pub fn capture_diff(repo: &Path, base: &str, out: &Path) -> Result<()> {
    git_raw(repo, &["add", "-A"])?;
    let o = git_raw(repo, &["--no-pager", "diff", "--cached", base])?;
    std::fs::write(out, &o.stdout)?;
    Ok(())
}

pub fn diffstat(repo: &Path, base: &str) -> String {
    git_raw(repo, &["--no-pager", "diff", "--stat", base])
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// Keep our scratch dir out of the user's tracked .gitignore. Creates
/// `.git/info/exclude` if it doesn't exist (some setups omit it), so `.duet/`
/// never shows up in `git status`.
pub fn ensure_ignored(repo: &Path) {
    use std::io::Write;
    let info = repo.join(".git/info");
    if !repo.join(".git").is_dir() {
        return; // worktree or non-standard layout; best-effort only
    }
    let ex = info.join("exclude");
    if std::fs::read_to_string(&ex).unwrap_or_default().lines().any(|l| l.trim() == ".duet/") {
        return;
    }
    let _ = std::fs::create_dir_all(&info);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&ex) {
        let _ = writeln!(f, ".duet/");
    }
}

/// Run the test command via `/bin/sh -c`, sending output to the log. Bounded by a
/// timeout (`DUET_TEST_TIMEOUT` seconds, default 600) so a hanging or interactive
/// test command can't block the verify phase forever.
pub fn run_test(repo: &Path, cmd: &str, log: &Path) -> Result<bool> {
    let out = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
    let err = out.try_clone()?;
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()?;
    let secs = std::env::var("DUET_TEST_TIMEOUT").ok().and_then(|v| v.parse().ok()).unwrap_or(600u64);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.success());
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!("test command timed out after {secs}s: {cmd}"));
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

/// Best-effort autodetection of a test command.
pub fn detect_test_cmd(repo: &Path) -> Option<String> {
    let has = |f: &str| repo.join(f).exists();
    if has("package.json") {
        if let Ok(s) = std::fs::read_to_string(repo.join("package.json")) {
            if s.contains("\"test\"") {
                return Some("npm test".into());
            }
        }
    }
    if has("Cargo.toml") {
        return Some("cargo test".into());
    }
    if has("go.mod") {
        return Some("go test ./...".into());
    }
    if has("pyproject.toml") || has("setup.py") || repo.join("tests").is_dir() {
        return Some("python3 -m pytest -q".into());
    }
    None
}
