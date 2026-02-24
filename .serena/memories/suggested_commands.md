# Suggested Commands (Linux)

## General shell / repo navigation
- `pwd`
- `ls`
- `cd /home/phantom/Documents/TitanClaw/titanclaw`
- `rg <pattern>` (preferred fast search)
- `rg --files`
- `find . -maxdepth 2 -type f`
- `git status`
- `git diff`

## Build / run (source)
- `cargo build --release`
- `cargo run` (default command behavior)
- `cargo run -- run`
- `cargo run -- onboard`
- `cargo run -- status`
- `cargo run -- doctor`
- `cargo run -- tool --help`
- `cargo run -- memory --help`
- `cargo run -- service --help`

## Built binary (from README examples)
- `./target/release/titanclaw onboard`
- `./target/release/titanclaw run`
- `./target/release/titanclaw status`
- `./target/release/titanclaw doctor`
- `./target/release/titanclaw memory bootstrap --dry-run`
- `./target/release/titanclaw memory bootstrap`

## Development checks
- `cargo fmt`
- `cargo clippy --all --benches --tests --examples --all-features`
- `cargo test`
- `cargo test --all-features`
- `cargo check`

## Project helper scripts
- `./scripts/dev-setup.sh` (environment setup + check/test flow)
- `./scripts/build-all.sh` (recommended before release builds when channel source packages changed)

## CI parity checks (observed in workflows)
- `cargo fmt --all -- --check`
- `cargo clippy -- -D warnings`
- `cargo test --verbose`
- `cargo test --all-features -- --nocapture`

## Optional experimental swarm run
- `SWARM_ENABLED=true SWARM_LISTEN_PORT=0 SWARM_HEARTBEAT_INTERVAL_SECS=15 SWARM_MAX_SLOTS=4 cargo run -- run`
