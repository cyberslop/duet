# Security Policy

## Reporting a vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

Instead, use GitHub's private vulnerability reporting:

1. Go to the [Security tab](https://github.com/cyberslop/duet/security) of this repository.
2. Click **Report a vulnerability** (or use this [direct link](https://github.com/cyberslop/duet/security/advisories/new)).

We aim to acknowledge reports within a few days and will keep you updated on the
fix and disclosure timeline. Once a fix is available, we'll coordinate a public
advisory and credit you unless you prefer to remain anonymous.

## Scope & threat model

`duet` is a local developer tool. It is worth keeping in mind that, by design, it:

- **drives other CLIs** (`claude`, `codex`) and a local OpenAI-compatible server,
  passing them prompts and your code;
- **runs in and reads from your working tree**, and writes run scratch to `.duet/`;
- **acts on a repository you point it at**.

Vulnerabilities we especially care about include: command/prompt injection that
escalates beyond the intended sandbox, paths that exfiltrate code or credentials
to an unintended destination, or writes outside the expected `.duet/` scratch and
target worktree. Issues in the upstream CLIs `duet` drives should be reported to
those projects, but please flag anything where `duet`'s handling makes them worse.

## Supported versions

`duet` is pre-1.0 and moves quickly. Security fixes land on `main`; please verify
against the latest `main` before reporting.
