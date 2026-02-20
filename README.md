<p align="center">
  <img src="ironclaw.png" alt="TitanClaw" width="220"/>
</p>

<h1 align="center">TitanClaw</h1>

<p align="center">
  <strong>IronClaw, upgraded for secure high-throughput orchestration.</strong>
</p>

<p align="center">
  <a href="#why-titanclaw">Why TitanClaw</a> •
  <a href="#titan-upgrade-status">Titan Upgrade Status</a> •
  <a href="#capabilities">Capabilities</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#security-model">Security Model</a>
</p>

---

## Why TitanClaw

TitanClaw is a security-first AI runtime built for people who need real automation, not just chat.

It keeps your data under your control, runs tools in hardened sandboxes, supports multi-provider inference, and scales execution through concurrent jobs and isolated workers.

## Titan Upgrade Status

Based on `implementation_plan.md`, this is where the upgrade stands today.

| Track | Status | What is live now |
|---|---|---|
| Phase 0: provider independence + local inference | ✅ | NEAR AI, OpenAI-compatible, Ollama, Tinfoil, provider failover |
| Phase 0: orchestration foundations | ✅ | Scheduler, parallel jobs, Docker worker/orchestrator flow |
| Phase 0: secure extensibility | ✅ | WASM tool system, dynamic tool building, secure skills framework |
| Phase 0: streaming everywhere | 🚧 | Gateway SSE/WebSocket exists; shell tool output now streams live per chunk, full token-to-tool piped execution still in progress |
| Phase 0: reflex fast-path bypass | 🚧 | Deterministic natural-language fast-path now bypasses LLM for high-confidence job intents (list/status/cancel/help/create) |
| Phase 1: deep context indexing | 🚧 | Tree-sitter and memory foundations are present; full GraphRAG path is not complete |
| Phase 2: distributed swarm mesh | 🔮 | libp2p dependencies are integrated; mesh-level runtime behavior is roadmap work |

## Capabilities

### What You Get Today

- Multi-provider LLM runtime with failover and retry logic
- Local-first memory with hybrid retrieval and persistent workspace context
- Secure WASM tool sandbox with capability gates and outbound allowlists
- Dynamic tool creation pipeline for runtime expansion
- Web gateway with WebSocket + SSE for real-time interaction
- Routines/automation engine for scheduled and event-driven tasks
- Docker-isolated workers for higher-risk or heavier executions
- OpenAI-compatible API endpoints for external integration
- LLM-bypassed fast-path for common job ops in natural language

### Built For Operators

- Strong defaults for prompt-injection resistance and secret handling
- Auditability and explicit approval flows for sensitive actions
- CLI + service model for persistent local operation
- Rust-native performance with a single deployable binary

## Quick Start

### Prerequisites

- Rust `1.92+`
- PostgreSQL `15+` with `pgvector` (recommended)
- Optional: Ollama for local inference

### Install

Use upstream release assets:

- Windows MSI: `https://github.com/nearai/ironclaw/releases/latest/download/ironclaw-x86_64-pc-windows-msvc.msi`
- PowerShell installer: `irm https://github.com/nearai/ironclaw/releases/latest/download/ironclaw-installer.ps1 | iex`
- Shell installer: `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/nearai/ironclaw/releases/latest/download/ironclaw-installer.sh | sh`

Or build from source:

```bash
git clone https://github.com/PhantomReaper2025/titanclaw.git
cd titanclaw
cargo build --release
```

### First Run

```bash
# Interactive setup wizard
./target/release/ironclaw onboard

# Start agent runtime (default command)
./target/release/ironclaw run
```

### Useful Commands

```bash
# Health and diagnostics
./target/release/ironclaw status
./target/release/ironclaw doctor

# Tool and memory management
./target/release/ironclaw tool --help
./target/release/ironclaw memory --help

# Service management
./target/release/ironclaw service --help
```

### Swarm Mesh (Experimental)

Enable the distributed Hive mesh runtime:

```bash
export SWARM_ENABLED=true
export SWARM_LISTEN_PORT=0
export SWARM_HEARTBEAT_INTERVAL_SECS=15
export SWARM_MAX_SLOTS=4
./target/release/ironclaw run
```

## Architecture

```text
Channels (CLI / Web / Webhooks / WASM Integrations)
          |
          v
Agent Loop + Router
          |
          +--> Scheduler (parallel task execution)
          |
          +--> Routines Engine (cron/event/webhook)
          |
          +--> Tool Registry (built-in + WASM + MCP)
          |
          +--> Orchestrator --> Docker Workers (isolated execution)
          |
          +--> Workspace + Hybrid Memory Store
```

For roadmap detail and rollout context, see `implementation_plan.md`.

## Security Model

Defense-in-depth is a core design constraint, not an add-on.

- WASM capability sandbox for untrusted tools
- Request/response scanning and policy checks for exfiltration patterns
- Secrets injected at host boundary instead of exposing raw credentials to tools
- Endpoint allowlisting for outbound network activity
- Isolation layers: in-process controls plus optional Docker worker boundaries

## Development

```bash
cargo fmt
cargo clippy --all --benches --tests --examples --all-features
cargo test
```

If you modify channel source packages, run `./scripts/build-all.sh` before a release build.

## Project Lineage

TitanClaw is built on IronClaw's Rust architecture and follows `implementation_plan.md` for upgrade execution.

## License

Licensed under either:

- Apache License, Version 2.0 (`LICENSE-APACHE`)
- MIT License (`LICENSE-MIT`)

at your option.
