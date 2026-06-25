<!--
Thanks for contributing to duet! Keep PRs focused and small where you can.
See CONTRIBUTING.md for conventions and the project layout.
-->

## What & why

<!-- What does this change do, and why? Link any related issue (e.g. "Closes #123"). -->

## How it was verified

<!-- Which tests/commands did you run? Note any live `duet` run if you touched the engine or a backend. -->

## Checklist

- [ ] `cargo test --workspace --locked` is green
- [ ] `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` is clean (zero warnings is the bar)
- [ ] Changed files are formatted (`cargo fmt -p <crate>`); the change stays focused
- [ ] New behavior comes with a test
- [ ] Commit subjects are clear and imperative
