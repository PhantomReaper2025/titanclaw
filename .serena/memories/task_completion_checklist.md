# Task Completion Checklist

When finishing work in TitanClaw, run/consider:

## Code quality and validation
- `cargo fmt`
- `cargo clippy --all --benches --tests --examples --all-features` (or at least CI-equivalent `cargo clippy -- -D warnings` when scope is smaller)
- `cargo test` (and `cargo test --all-features` if feature-sensitive changes were made)
- If channel source packages changed, run `./scripts/build-all.sh` before release build work

## Project policy checks (from `AGENTS.md` / `CONTRIBUTING.md`)
- If implementation status or planned capability status changed, update `implementation_plan.md`
- If feature behavior changed, sync canonical docs in same branch:
  - `AGENTS.md`
  - `IDENTITY.md`
  - `README.md`
  - `implementation_plan.md`
  - `CHANGELOG.md`
- Ensure sandbox job completion uses structured completion signaling as primary path (free-text completion only fallback)

## Practical final checks
- `git status` to confirm touched files
- Review diffs (`git diff`) for accidental changes
- Mention any tests/checks not run and why
